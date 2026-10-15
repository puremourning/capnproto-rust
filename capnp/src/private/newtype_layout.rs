//! Where `type` newtypes put their members at each use site, computed from schema nodes.
//!
//! A group or union newtype (`type Vec3 = group { ... }`) is a `type` node whose target is a
//! *template* struct node owned by the newtype (its `scopeId` is the `type` node). Each field
//! declared with the newtype stamps a copy of the template into its parent as a group (the *use
//! site*), and the compiler places the copy's members wherever they fit. The functions here work
//! out, from the nodes alone, which newtypes a field was declared with and where a use site put
//! the template's members.
//!
//! The code generator renders the results as `static`s ([`crate::traits::UseSiteTable`]) and
//! `RawStructSchema` builder calls; a run-time schema loader can turn them into the same
//! tables with [`LayoutTable::leak`]. Because both compute them here, a loaded use site and a
//! compiled one agree by construction.
//!
//! Not a stable API: this exists for generated code and `capnpc`.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::schema_capnp::{field, node, type_};
use crate::traits::{UseSiteEntry, UseSiteTable, EMPTY_USE_SITE_TABLE, UNMAPPED};
use crate::Result;

/// Finds schema nodes by id: the closed set of nodes a layout is computed over (for example a
/// `CodeGeneratorRequest`'s `nodes`). A missing node is treated as "not a newtype" rather than
/// an error, as the code generator always has.
pub trait NodeLookup<'a> {
    fn node(&self, id: u64) -> Option<node::Reader<'a>>;
}

impl<'a, T: NodeLookup<'a> + ?Sized> NodeLookup<'a> for &T {
    fn node(&self, id: u64) -> Option<node::Reader<'a>> {
        (**self).node(id)
    }
}

#[cfg(feature = "std")]
impl<'a, S: core::hash::BuildHasher> NodeLookup<'a>
    for std::collections::HashMap<u64, node::Reader<'a>, S>
{
    fn node(&self, id: u64) -> Option<node::Reader<'a>> {
        self.get(&id).copied()
    }
}

impl<'a> NodeLookup<'a> for BTreeMap<u64, node::Reader<'a>> {
    fn node(&self, id: u64) -> Option<node::Reader<'a>> {
        self.get(&id).copied()
    }
}

/// How deeply newtype members may nest before the input is considered malformed. Valid schemas
/// nest only as deep as their source does; the bound stops a cyclic node set from recursing
/// forever.
const MAX_NESTING: u32 = 64;

/// The layout of one use site of a group or union newtype: what
/// `RawStructSchema::with_any_layout` records for the use-site group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseSiteLayout {
    /// The newtypes the use-site field was declared with, nearest first; see
    /// [`field_newtype_ids`].
    pub newtype_ids: Vec<u64>,
    /// Where the use site put the template's members.
    pub table: LayoutTable,
}

/// The owned form of a [`UseSiteTable`]: one level of a use site's layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutTable {
    /// Indexed by the member's position in the template's `fields` list.
    pub entries: Vec<LayoutEntry>,
    /// The use-site group's discriminant offset (meaningful for union newtypes only).
    pub discriminant_offset: u32,
}

/// The owned form of a [`UseSiteEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutEntry {
    /// A slot member's offset at the use site, or [`UNMAPPED`].
    Offset(u32),
    /// A member that is itself a nested newtype. `None` if it is unmapped at this use site (it
    /// then reads through [`EMPTY_USE_SITE_TABLE`]).
    Nested(Option<NestedLayout>),
}

/// A nested-newtype member's own use site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedLayout {
    /// The id of the nested use-site group node (a child of the enclosing use site).
    pub group_id: u64,
    pub table: LayoutTable,
}

impl LayoutTable {
    /// Converts to a [`UseSiteTable`] by leaking its allocations. Meant for a schema loader,
    /// whose schemas live forever anyway.
    pub fn leak(self) -> &'static UseSiteTable<'static> {
        Box::leak(Box::new(self.into_use_site_table()))
    }

    fn into_use_site_table(self) -> UseSiteTable<'static> {
        let entries: Vec<UseSiteEntry<'static>> = self
            .entries
            .into_iter()
            .map(|entry| match entry {
                LayoutEntry::Offset(offset) => UseSiteEntry::Offset(offset),
                LayoutEntry::Nested(None) => UseSiteEntry::Nested(&EMPTY_USE_SITE_TABLE),
                LayoutEntry::Nested(Some(nested)) => UseSiteEntry::Nested(nested.table.leak()),
            })
            .collect();
        UseSiteTable {
            entries: Box::leak(entries.into_boxed_slice()),
            discriminant_offset: self.discriminant_offset,
        }
    }
}

impl PartialEq<UseSiteTable<'_>> for LayoutTable {
    /// Structural equality with a [`UseSiteTable`], as [`leak`](Self::leak) would produce it.
    fn eq(&self, other: &UseSiteTable<'_>) -> bool {
        self.discriminant_offset == other.discriminant_offset
            && self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(other.entries)
                .all(|(a, b)| match (a, b) {
                    (LayoutEntry::Offset(a), UseSiteEntry::Offset(b)) => a == b,
                    (LayoutEntry::Nested(None), UseSiteEntry::Nested(b)) => {
                        EMPTY_USE_SITE_TABLE == **b
                    }
                    (LayoutEntry::Nested(Some(a)), UseSiteEntry::Nested(b)) => a.table == **b,
                    _ => false,
                })
    }
}

/// What `RawStructSchema::with_any_layout` recorded on `schema` (generated code records it on
/// use-site groups): the newtype ids and the use-site table. `(&[], None)` for any other struct.
/// For checking a layout computed here against a compiled one.
pub fn recorded_layout(
    schema: crate::schema::StructSchema,
) -> (&'static [u64], Option<&'static UseSiteTable<'static>>) {
    (schema.raw.generic.newtype_ids, schema.raw.generic.any_table)
}

/// The `type` newtypes a field was declared with, nearest first. A field declared `corner
/// :ChainedPoint`, where `type ChainedPoint = Point` and `type Point = Vec3`, gives
/// `[ChainedPoint, Point, Vec3]`. Empty for a field not declared with a newtype.
///
/// A group field records its newtype in `Field.typeId`; a slot field in its type's
/// `Type.typeId`. Each `type` node records the newtype it aliases (if any) in its own
/// `Type.typeId`, which is how the chain is followed. A newtype whose node isn't in `nodes` ends
/// the chain there.
pub fn field_newtype_ids<'a>(
    nodes: &impl NodeLookup<'a>,
    field: field::Reader<'_>,
) -> Result<Vec<u64>> {
    let mut id = field.get_type_id();
    if id == 0 {
        if let field::Slot(slot) = field.which()? {
            id = slot.get_type()?.get_type_id();
        }
    }
    let mut ids = Vec::new();
    // The schema compiler rejects alias cycles; the `contains` check only stops a malformed
    // node set from looping forever.
    while id != 0 && !ids.contains(&id) {
        ids.push(id);
        let Some(node) = nodes.node(id) else {
            break;
        };
        let node::Type(Ok(t)) = node.which()? else {
            break;
        };
        id = t.get_type_id();
    }
    Ok(ids)
}

/// The template node id of the inline group or union newtype `alias_id`, or of the inline
/// newtype it aliases (`type Bar = Foo` shares Foo's template). `None` for anything else: a
/// scalar newtype, a newtype naming a pre-existing struct, a non-`type` node, or a missing one.
pub fn newtype_template_id<'a>(nodes: &impl NodeLookup<'a>, alias_id: u64) -> Result<Option<u64>> {
    let mut alias_id = alias_id;
    let mut seen = Vec::new();
    loop {
        if seen.contains(&alias_id) {
            return Ok(None); // an alias cycle; the schema compiler rejects these
        }
        seen.push(alias_id);
        let Some(alias) = nodes.node(alias_id) else {
            return Ok(None);
        };
        let node::Type(Ok(t)) = alias.which()? else {
            return Ok(None);
        };
        let type_::Struct(st) = t.which()? else {
            return Ok(None); // scalar newtype
        };
        let template_id = st.get_type_id();
        let Some(template) = nodes.node(template_id) else {
            return Ok(None);
        };
        if template.get_scope_id() == alias_id {
            return Ok(Some(template_id));
        }
        // Not this node's own template. Either it aliases another inline newtype (recorded as
        // Type.typeId), whose template it shares, or it names a pre-existing struct.
        let aliased = t.get_type_id();
        if aliased == 0 {
            return Ok(None);
        }
        alias_id = aliased;
    }
}

/// The inline newtype owning `alias_id`'s template: `alias_id` itself, or for `type Bar = Foo`
/// (Foo an inline group/union newtype), Foo. `None` if `alias_id` isn't an inline newtype.
pub fn inline_newtype_owner<'a>(nodes: &impl NodeLookup<'a>, alias_id: u64) -> Result<Option<u64>> {
    Ok(newtype_template_id(nodes, alias_id)?
        .and_then(|t| nodes.node(t))
        .map(|t| t.get_scope_id()))
}

/// The template of a newtype whose template is a struct with the given union-ness, and whose
/// group members (if any) are all nested newtypes.
fn template_id_of_kind<'a>(
    nodes: &impl NodeLookup<'a>,
    alias_id: u64,
    union: bool,
) -> Result<Option<u64>> {
    let Some(template_id) = newtype_template_id(nodes, alias_id)? else {
        return Ok(None);
    };
    let Some(template) = nodes.node(template_id) else {
        return Ok(None);
    };
    let node::Struct(tmpl) = template.which()? else {
        return Ok(None);
    };
    if (tmpl.get_discriminant_count() > 0) != union {
        return Ok(None);
    }
    for field in tmpl.get_fields()? {
        if let field::Group(_) = field.which()? {
            if field.get_type_id() == 0 {
                return Ok(None); // anonymous group member -- not a newtype, not supported
            }
        }
    }
    Ok(Some(template_id))
}

/// The template of the non-union group newtype `alias_id` (or an alias of one). Its fields are
/// slots and/or nested-newtype members (group fields carrying a `Field.typeId`). `None` if the
/// template has an anonymous group member, which is not supported.
pub fn group_newtype_template_id<'a>(
    nodes: &impl NodeLookup<'a>,
    alias_id: u64,
) -> Result<Option<u64>> {
    template_id_of_kind(nodes, alias_id, false)
}

/// The template of the union newtype `alias_id` (or an alias of one). Its arms are slots or
/// nested newtypes. `None` if the template has an anonymous group arm, which is not supported.
pub fn union_newtype_template_id<'a>(
    nodes: &impl NodeLookup<'a>,
    alias_id: u64,
) -> Result<Option<u64>> {
    template_id_of_kind(nodes, alias_id, true)
}

/// Whether a use site of `alias_id` gets a use-site table (and erased carriers): true for a
/// group or union newtype, or an alias of one.
pub fn has_use_site_table<'a>(nodes: &impl NodeLookup<'a>, alias_id: u64) -> Result<bool> {
    Ok(group_newtype_template_id(nodes, alias_id)?.is_some()
        || union_newtype_template_id(nodes, alias_id)?.is_some())
}

/// The table of the use-site group `instance_id` of the newtype whose template is
/// `template_id`. It has one entry per template field, indexed by the field's position in the
/// template's `fields` list (stable under evolution). A slot maps to the offset of the
/// same-named instance slot, or to [`UNMAPPED`] (an incomplete `@[...]`). A nested-newtype member
/// gets the nested use site's own table. `None` if either node is missing or not a struct.
pub fn use_site_table<'a>(
    nodes: &impl NodeLookup<'a>,
    template_id: u64,
    instance_id: u64,
) -> Result<Option<LayoutTable>> {
    use_site_table_at(nodes, template_id, instance_id, 0)
}

fn use_site_table_at<'a>(
    nodes: &impl NodeLookup<'a>,
    template_id: u64,
    instance_id: u64,
    depth: u32,
) -> Result<Option<LayoutTable>> {
    if depth > MAX_NESTING {
        return Err(crate::Error::failed(
            "newtype use sites nest too deeply (cyclic schema nodes?)".into(),
        ));
    }
    let Some(node::Struct(tmpl)) = nodes.node(template_id).map(|n| n.which()).transpose()? else {
        return Ok(None);
    };
    let Some(node::Struct(instance)) = nodes.node(instance_id).map(|n| n.which()).transpose()?
    else {
        return Ok(None);
    };
    let mut inst_fields = BTreeMap::new();
    for f in instance.get_fields()? {
        inst_fields.insert(f.get_name()?.to_str()?, f);
    }
    let mut entries = Vec::new();
    for field in tmpl.get_fields()? {
        let inst = inst_fields
            .get(field.get_name()?.to_str()?)
            .map(|f| f.which());
        let entry = match (field.which()?, inst) {
            (field::Slot(_), Some(Ok(field::Slot(s)))) => LayoutEntry::Offset(s.get_offset()),
            (field::Slot(_), _) => LayoutEntry::Offset(UNMAPPED),
            (field::Group(_), inst) => {
                let alias_id = field.get_type_id();
                let nested = match inst {
                    Some(Ok(field::Group(ig))) if has_use_site_table(nodes, alias_id)? => {
                        let group_id = ig.get_type_id();
                        // `has_use_site_table` found the template, and `group_id` is an
                        // instance field's group, so both are present and structs unless the
                        // node set is malformed.
                        let table = match newtype_template_id(nodes, alias_id)? {
                            Some(t) => use_site_table_at(nodes, t, group_id, depth + 1)?,
                            None => None,
                        };
                        table.map(|table| NestedLayout { group_id, table })
                    }
                    _ => None,
                };
                LayoutEntry::Nested(nested)
            }
        };
        entries.push(entry);
    }
    Ok(Some(LayoutTable {
        entries,
        discriminant_offset: instance.get_discriminant_offset(),
    }))
}

/// For a group node that is a use site of a group or union newtype, its layout: the newtype ids
/// of the field declaring it and its use-site table. `None` for any other node (including a use
/// site of a newtype without a use-site table, see [`has_use_site_table`]).
pub fn use_site_layout<'a>(
    nodes: &impl NodeLookup<'a>,
    group_node: node::Reader<'_>,
) -> Result<Option<UseSiteLayout>> {
    let node::Struct(instance) = group_node.which()? else {
        return Ok(None);
    };
    if !instance.get_is_group() {
        return Ok(None);
    }
    let id = group_node.get_id();
    let Some(parent) = nodes.node(group_node.get_scope_id()) else {
        return Ok(None);
    };
    let node::Struct(parent_struct) = parent.which()? else {
        return Ok(None);
    };
    for field in parent_struct.get_fields()? {
        let field::Group(group) = field.which()? else {
            continue;
        };
        if group.get_type_id() != id || field.get_type_id() == 0 {
            continue;
        }
        let alias_id = field.get_type_id();
        if !has_use_site_table(nodes, alias_id)? {
            return Ok(None);
        }
        let Some(template_id) = newtype_template_id(nodes, alias_id)? else {
            return Ok(None);
        };
        let Some(table) = use_site_table(nodes, template_id, id)? else {
            return Ok(None);
        };
        return Ok(Some(UseSiteLayout {
            newtype_ids: field_newtype_ids(nodes, field)?,
            table,
        }));
    }
    Ok(None)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::message;
    use crate::struct_list;

    // `LayoutTable::leak` leaks on purpose. Keeping the result reachable from a static stops Miri
    // from reporting it.
    static KEPT: std::sync::Mutex<Vec<&'static UseSiteTable<'static>>> =
        std::sync::Mutex::new(Vec::new());

    fn leak(table: LayoutTable) -> &'static UseSiteTable<'static> {
        let leaked = table.leak();
        KEPT.lock().unwrap().push(leaked);
        leaked
    }

    const FILE: u64 = 1;
    const VEC2: u64 = 10; // type Vec2 = group { x @0 :Float32; y @1 :Float32; }
    const VEC2_TEMPLATE: u64 = 11;
    const POINT: u64 = 12; // type Point = Vec2;
    const SHAPE: u64 = 20; // struct Shape { at @[4] :Point; }  (y unmapped)
    const SHAPE_AT: u64 = 21;
    const CYCLE_A: u64 = 30; // malformed: two `type` nodes aliasing each other
    const CYCLE_B: u64 = 31;

    fn slot(mut field: field::Builder<'_>, name: &str, offset: u32) {
        field.set_name(name);
        let mut slot = field.init_slot();
        slot.set_offset(offset);
        slot.init_type().set_float32(());
    }

    fn build(nodes: &mut struct_list::Builder<'_, node::Owned>) {
        let mut file = nodes.reborrow().get(0);
        file.set_id(FILE);
        file.set_file(());

        let mut vec2 = nodes.reborrow().get(1);
        vec2.set_id(VEC2);
        vec2.set_scope_id(FILE);
        vec2.init_type().init_struct().set_type_id(VEC2_TEMPLATE);

        let mut template = nodes.reborrow().get(2);
        template.set_id(VEC2_TEMPLATE);
        template.set_scope_id(VEC2);
        let mut fields = template.init_struct().init_fields(2);
        slot(fields.reborrow().get(0), "x", 0);
        slot(fields.reborrow().get(1), "y", 1);

        let mut point = nodes.reborrow().get(3);
        point.set_id(POINT);
        point.set_scope_id(FILE);
        let mut ty = point.init_type();
        ty.set_type_id(VEC2);
        ty.init_struct().set_type_id(VEC2_TEMPLATE);

        let mut shape = nodes.reborrow().get(4);
        shape.set_id(SHAPE);
        shape.set_scope_id(FILE);
        let mut at = shape.init_struct().init_fields(1).get(0);
        at.set_name("at");
        at.set_type_id(POINT);
        at.init_group().set_type_id(SHAPE_AT);

        let mut shape_at = nodes.reborrow().get(5);
        shape_at.set_id(SHAPE_AT);
        shape_at.set_scope_id(SHAPE);
        let mut st = shape_at.init_struct();
        st.set_is_group(true);
        slot(st.init_fields(1).get(0), "x", 4);

        for (i, (id, aliased)) in [(CYCLE_A, CYCLE_B), (CYCLE_B, CYCLE_A)]
            .into_iter()
            .enumerate()
        {
            let mut n = nodes.reborrow().get(6 + i as u32);
            n.set_id(id);
            n.set_scope_id(FILE);
            let mut ty = n.init_type();
            ty.set_type_id(aliased);
            ty.init_struct().set_type_id(SHAPE); // a struct the alias doesn't own
        }
    }

    #[test]
    fn layout_from_hand_built_nodes() {
        let mut message = message::Builder::new_default();
        let mut list: struct_list::Builder<'_, node::Owned> = message.initn_root(8);
        build(&mut list);
        let list = list.into_reader();
        let mut nodes = BTreeMap::new();
        for node in list.iter() {
            nodes.insert(node.get_id(), node);
        }

        assert_eq!(
            newtype_template_id(&nodes, VEC2).unwrap(),
            Some(VEC2_TEMPLATE)
        );
        assert_eq!(
            newtype_template_id(&nodes, POINT).unwrap(),
            Some(VEC2_TEMPLATE)
        );
        assert_eq!(inline_newtype_owner(&nodes, POINT).unwrap(), Some(VEC2));
        assert_eq!(
            group_newtype_template_id(&nodes, POINT).unwrap(),
            Some(VEC2_TEMPLATE)
        );
        assert_eq!(union_newtype_template_id(&nodes, POINT).unwrap(), None);
        assert!(has_use_site_table(&nodes, POINT).unwrap());
        assert!(!has_use_site_table(&nodes, SHAPE).unwrap());
        assert_eq!(newtype_template_id(&nodes, 999).unwrap(), None);

        // Alias cycles end instead of looping.
        assert_eq!(newtype_template_id(&nodes, CYCLE_A).unwrap(), None);

        let node::Struct(shape) = nodes[&SHAPE].which().unwrap() else {
            panic!()
        };
        let at = shape.get_fields().unwrap().get(0);
        assert_eq!(field_newtype_ids(&nodes, at).unwrap(), [POINT, VEC2]);

        let expected = UseSiteLayout {
            newtype_ids: alloc::vec![POINT, VEC2],
            table: LayoutTable {
                entries: alloc::vec![LayoutEntry::Offset(4), LayoutEntry::Offset(UNMAPPED)],
                discriminant_offset: 0,
            },
        };
        let layout = use_site_layout(&nodes, nodes[&SHAPE_AT]).unwrap();
        assert_eq!(layout.as_ref(), Some(&expected));
        assert_eq!(use_site_layout(&nodes, nodes[&SHAPE]).unwrap(), None);
        assert_eq!(
            use_site_layout(&nodes, nodes[&VEC2_TEMPLATE]).unwrap(),
            None
        );

        let leaked = leak(expected.table.clone());
        assert_eq!(leaked.offset(0), 4);
        assert_eq!(leaked.offset(1), UNMAPPED);
        assert_eq!(expected.table, *leaked);
        // The leaked table is what generated code would emit for this use site.
        static GENERATED: UseSiteTable<'static> = UseSiteTable {
            entries: &[UseSiteEntry::Offset(4), UseSiteEntry::Offset(0xffff_ffff)],
            discriminant_offset: 0,
        };
        assert_eq!(*leaked, GENERATED);
    }
}
