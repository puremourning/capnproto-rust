//! Loading schemas at run time.
//!
//! A [`SchemaLoader`] takes schema nodes that are not compiled into the binary, typically the
//! `nodes` of a `CodeGeneratorRequest` produced by `capnp compile -o-`, and turns them into
//! [`StructSchema`] / [`EnumSchema`] / [`Type`] values. Those drive the dynamic API
//! ([`crate::dynamic_struct`], [`crate::dynamic_list`], [`crate::dynamic_value`] and their
//! `Debug` output) exactly as compiled-in schemas do.
//!
//! Point the result at bytes with [`message::Reader::get_root_as_dynamic`],
//! [`any_pointer::Reader::get_as_dynamic`](crate::any_pointer::Reader::get_as_dynamic) or the
//! builder equivalents ([`message::Builder::init_root_dynamic`] and so on). How a program learns
//! which type a message holds, and where its schema comes from, is up to it; `example/dynamic-dump`
//! in the repository shows one convention, an envelope carrying a type id and, optionally, the
//! schema nodes.
//!
//! # Example
//!
//! The request would normally come from `capnp compile -o- point.capnp > point.cgr`, read with
//! [`serialize::read_message`](crate::serialize::read_message). Here it is built by hand, for
//! this schema:
//!
//! ```capnp
//! struct Point { x @0 :Int32; y @1 :Int32; }
//! ```
//!
//! ```
//! use capnp::message;
//! use capnp::schema_capnp::code_generator_request;
//! use capnp::schema_loader::SchemaLoader;
//! # use capnp::schema_capnp::{field, node};
//! # const POINT_ID: u64 = 0xd1b2_0dca_6b1d_9a01;
//! # fn point_cgr() -> message::Builder<message::HeapAllocator> {
//! #     let mut message = message::Builder::new_default();
//! #     let cgr: code_generator_request::Builder = message.init_root();
//! #     let mut point = cgr.init_nodes(1).get(0);
//! #     point.set_id(POINT_ID);
//! #     point.set_display_name("point.capnp:Point");
//! #     point.set_display_name_prefix_length(12);
//! #     let mut st = point.init_struct();
//! #     st.set_data_word_count(1);
//! #     st.set_preferred_list_encoding(capnp::schema_capnp::ElementSize::EightBytes);
//! #     let mut fields = st.init_fields(2);
//! #     for (i, name) in ["x", "y"].into_iter().enumerate() {
//! #         let mut f = fields.reborrow().get(i as u32);
//! #         f.set_name(name);
//! #         f.set_code_order(i as u16);
//! #         f.set_discriminant_value(field::NO_DISCRIMINANT);
//! #         f.reborrow().init_ordinal().set_explicit(i as u16);
//! #         let mut slot = f.init_slot();
//! #         slot.set_offset(i as u32);
//! #         slot.reborrow().init_type().set_int32(());
//! #         slot.init_default_value().set_int32(0);
//! #     }
//! #     message
//! # }
//!
//! # fn main() -> capnp::Result<()> {
//! let cgr_message = point_cgr();
//! let cgr: code_generator_request::Reader = cgr_message.get_root_as_reader()?;
//!
//! let loader = SchemaLoader::new();
//! loader.load_code_generator_request(cgr)?;
//! let schema = loader.get_struct(POINT_ID)?;
//!
//! // Write a Point through the dynamic API...
//! let mut message = message::Builder::new_default();
//! let mut point = message.init_root_dynamic(schema)?;
//! point.set_named("x", 3i32.into())?;
//! point.set_named("y", (-4i32).into())?;
//!
//! // ...and read it back.
//! let message = message.into_reader();
//! let point = message.get_root_as_dynamic(schema)?;
//! assert_eq!(point.get_named("y")?.downcast::<i32>(), -4);
//! assert_eq!(format!("{point:?}"), "(x = 3, y = -4)");
//! # Ok(())
//! # }
//! ```
//!
//! # Validation
//!
//! Loading validates its input, which may be untrusted. Every node of a batch is checked so
//! that nothing in [`crate::schema`] or the dynamic API can panic or loop forever on it: member
//! counts and discriminants, field offsets against the declared section sizes, default values
//! against field types, group nodes against their parents, generic parameter references, alias
//! chains, and every node a batch references (field types, brand bindings, groups,
//! annotations, newtypes) must be loaded, or in the batch, and be of the right kind. If any
//! check fails, the whole batch fails and nothing is loaded.
//!
//! # Identity
//!
//! Each instantiation of a generic struct (`Foo(Text)`) is interned, so looking it up twice in
//! one loader gives equal schemas. A loaded schema is never equal to a compiled one, and two
//! loaders' schemas are never equal to each other, even for the same node.
//!
//! # Memory
//!
//! Loaded schemas are `'static`, like compiled ones, so the loader never frees what it
//! allocates, even when it is dropped. Reloading a node with identical content reuses the
//! existing allocation, so memory grows only with the number of distinct nodes loaded. A
//! dropped loader's schemas stay reachable from a process-wide registry, which is what keeps
//! them alive (and keeps leak checkers such as Miri quiet).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::introspect::{
    self, AnnotationSource, BrandScope, Introspect, RawBrandedStructSchema, RawEnumSchema,
    RawStructSchema, StructBrand, Type, TypeVariant,
};
use crate::private::arena::GeneratedCodeArena;
use crate::private::layout;
use crate::private::newtype_layout::{self, NodeLookup, UseSiteLayout};
use crate::schema::{EnumSchema, StructSchema};
use crate::schema_capnp::{annotation, brand, code_generator_request, field, node, type_, value};
use crate::{message, Error, Result, Word};

/// Every loader's state, so that what the loaders leak stays reachable.
static LOADERS: Mutex<Vec<&'static Mutex<Inner>>> = Mutex::new(Vec::new());

/// How deeply a schema `Type` may nest (`List(List(...))`, brand bindings) before it is
/// rejected. The canonical copy's reader limits nesting to 64 anyway.
const MAX_TYPE_DEPTH: u32 = 128;

/// Loads schema nodes at run time; see the [module documentation](self).
pub struct SchemaLoader {
    inner: &'static Mutex<Inner>,
}

impl Default for SchemaLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for SchemaLoader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let inner = lock(self.inner);
        f.debug_struct("SchemaLoader")
            .field("nodes", &inner.nodes.len())
            .field("instances", &inner.instances.len())
            .finish()
    }
}

impl SchemaLoader {
    /// Constructs a loader with nothing loaded.
    pub fn new() -> Self {
        let inner: &'static Mutex<Inner> = Box::leak(Box::new(Mutex::new(Inner::default())));
        LOADERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(inner);
        Self { inner }
    }

    /// Validates, copies and resolves a batch of nodes.
    ///
    /// The batch plus everything previously loaded must be closed: a reference to a node that
    /// is neither fails the whole batch, and nothing is loaded. Reloading a node with the same
    /// content is a no-op; reloading an id with different content is an error.
    pub fn load<'n>(&self, nodes: impl IntoIterator<Item = node::Reader<'n>>) -> Result<()> {
        let mut copies = Vec::new();
        for node in nodes {
            copies.push((node.get_id(), canonical_copy(node)?));
        }
        lock(self.inner).load_batch(self.inner, copies)
    }

    /// Loads every node of a `CodeGeneratorRequest` (`capnp compile -o- foo.capnp`). Such a
    /// request contains every node the compiler parsed, imports included, so it is closed.
    pub fn load_code_generator_request(
        &self,
        cgr: code_generator_request::Reader<'_>,
    ) -> Result<()> {
        self.load(cgr.get_nodes()?)
    }

    /// The struct with id `id`. Generic parameters are unbound, which reads as `AnyPointer`.
    pub fn get_struct(&self, id: u64) -> Result<StructSchema> {
        let inner = lock(self.inner);
        let brand = inner.default_brand(id)?;
        match brand.node.raw {
            Some(_) => Ok(StructSchema::new(brand.raw_struct())),
            None => Err(wrong_kind(id, "a struct")),
        }
    }

    /// The enum with id `id`.
    pub fn get_enum(&self, id: u64) -> Result<EnumSchema> {
        let inner = lock(self.inner);
        let brand = inner.default_brand(id)?;
        match brand.node.kind {
            Kind::Enum => Ok(EnumSchema::new(brand.raw_enum())),
            _ => Err(wrong_kind(id, "an enum")),
        }
    }

    /// The node with id `id`, of any kind (interfaces, constants, annotations, ... included).
    pub fn get_node(&self, id: u64) -> Result<node::Reader<'static>> {
        Ok(lock(self.inner).loaded(id)?.proto())
    }

    /// The struct with id `id`, branded with `brand`, as in a `Type.struct` of a schema node.
    /// Parameters `brand` binds to other parameters, or leaves to be inherited, take their
    /// bindings from `scope` (unbound, so `AnyPointer`, without one).
    pub fn get_struct_branded(
        &self,
        id: u64,
        brand: brand::Reader<'_>,
        scope: Option<StructSchema>,
    ) -> Result<StructSchema> {
        let (scopes, bindings) = scope_bindings(scope);
        let ctx = Ctx {
            scopes: &scopes,
            bindings: &bindings,
        };
        let mut inner = lock(self.inner);
        let node = inner.loaded(id)?;
        if node.raw.is_none() {
            return Err(wrong_kind(id, "a struct"));
        }
        check_brand(&*inner, brand, id, &[], false, 0)?;
        let Some(bindings) = inner.resolve_brand(id, brand, ctx, true)? else {
            unreachable!("interning always resolves")
        };
        let Some(instance) = inner.instance(id, bindings, true)? else {
            unreachable!("interning always resolves")
        };
        Ok(StructSchema::new(instance.raw_struct()))
    }

    /// Resolves a schema `Type`, for example one read from an annotation or from a user's own
    /// envelope struct. Generic parameters take their bindings from `scope` (`AnyPointer`
    /// without one).
    pub fn get_type(&self, ty: type_::Reader<'_>, scope: Option<StructSchema>) -> Result<Type> {
        let (scopes, bindings) = scope_bindings(scope);
        let ctx = Ctx {
            scopes: &scopes,
            bindings: &bindings,
        };
        let mut inner = lock(self.inner);
        check_type(&*inner, ty, &[], false, 0)?;
        match inner.resolve_type(ty, ctx, true)? {
            Some(t) => Ok(t),
            None => unreachable!("interning always resolves"),
        }
    }
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    // Nothing panics while the lock is held on valid input, but if something did, the maps are
    // still consistent: entries are only ever added, after they are complete.
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

fn any_pointer() -> Type {
    crate::any_pointer::Owned::introspect()
}

fn failed(message: String) -> Error {
    Error::failed(message)
}

fn missing(id: u64) -> Error {
    failed(format!("schema loader: no node with id {id:#x} is loaded"))
}

fn wrong_kind(id: u64, expected: &str) -> Error {
    failed(format!("schema loader: node {id:#x} is not {expected}"))
}

/// The bindings a scope schema supplies to a lookup: its brand scopes and their bindings.
fn scope_bindings(scope: Option<StructSchema>) -> (Vec<BrandScope>, Vec<Type>) {
    let Some(scope) = scope else {
        return (Vec::new(), Vec::new());
    };
    let scopes = scope.get_brand_scopes().to_vec();
    let mut bindings = Vec::new();
    for s in &scopes {
        for i in 0..s.param_count() {
            bindings.push(
                scope
                    .get_brand_binding(s.scope_id(), i)
                    .unwrap_or_else(any_pointer),
            );
        }
    }
    (scopes, bindings)
}

/// Copies `node` into a fresh, canonical, single-segment message.
fn canonical_copy(node: node::Reader<'_>) -> Result<Vec<Word>> {
    const MAX_WORDS: u64 = 1 << 29;
    let size = node.total_size()?.word_count + 1;
    let size = match u32::try_from(size) {
        Ok(size) if u64::from(size) <= MAX_WORDS => size,
        _ => {
            return Err(failed(format!(
                "schema loader: node {:#x} is too large",
                node.get_id()
            )))
        }
    };
    let mut message =
        message::Builder::new(message::HeapAllocator::new().first_segment_words(size));
    message.set_root_canonical::<node::Owned>(node)?;
    let segments = message.get_segments_for_output();
    let bytes = segments[0];
    let mut words = Word::allocate_zeroed_vec(bytes.len() / crate::private::units::BYTES_PER_WORD);
    Word::words_to_bytes_mut(&mut words).copy_from_slice(bytes);
    Ok(words)
}

fn leak_slice<T>(v: Vec<T>) -> &'static [T] {
    if v.is_empty() {
        &[]
    } else {
        Box::leak(v.into_boxed_slice())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Struct,
    Enum,
    Interface,
    Const,
    Annotation,
    Type,
}

impl Kind {
    fn of(node: node::Reader<'_>) -> Result<Self> {
        Ok(match node.which()? {
            node::File(()) => Kind::File,
            node::Struct(_) => Kind::Struct,
            node::Enum(_) => Kind::Enum,
            node::Interface(_) => Kind::Interface,
            node::Const(_) => Kind::Const,
            node::Annotation(_) => Kind::Annotation,
            node::Type(_) => Kind::Type,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Kind::File => "a file",
            Kind::Struct => "a struct",
            Kind::Enum => "an enum",
            Kind::Interface => "an interface",
            Kind::Const => "a const",
            Kind::Annotation => "an annotation",
            Kind::Type => "a `type` newtype",
        }
    }
}

/// A loaded node: a canonical copy of its encoding, which is also the arena its schemas read
/// from, and what is computed from it once.
pub(crate) struct LoadedNode {
    words: &'static [Word],
    arena: &'static GeneratedCodeArena,
    kind: Kind,
    /// The id of the node's parent scope: its `scopeId`, or for a method's implicit parameter
    /// or result struct, the interface. 0 for none.
    parent: u64,
    /// The generic scopes the node binds parameters of, outermost first; see
    /// [`StructSchema::get_brand_scopes`].
    scopes: &'static [BrandScope],
    /// For a struct node.
    raw: Option<&'static RawStructSchema>,
    loader: &'static Mutex<Inner>,
}

impl LoadedNode {
    fn proto(&self) -> node::Reader<'static> {
        let arena: &'static GeneratedCodeArena = self.arena;
        // The words are a canonical copy made through the bounds-checked reader, and were read
        // as a node before they were loaded, so this can't fail.
        crate::any_pointer::Reader::new(
            layout::PointerReader::get_root_from_arena(arena).expect("loaded node arena"),
        )
        .get_as()
        .expect("loaded node")
    }

    fn param_count(&self) -> usize {
        self.scopes
            .iter()
            .map(|s| usize::from(s.param_count()))
            .sum()
    }
}

/// A type in a loaded instance's table.
enum LazyType {
    Resolved(Type),
    /// A type naming a generic instantiation that wasn't interned when the table was built.
    /// The set of instantiations reachable from a schema can be infinite (polymorphic
    /// recursion), so they are interned when first reached. Load validated everything this
    /// depends on, so filling it can't fail.
    Deferred(OnceLock<Type>),
}

struct AnnotationTypes {
    /// The node's own annotations.
    node: Box<[LazyType]>,
    /// For each field (struct) or enumerant (enum), its annotations.
    children: Box<[Box<[LazyType]>]>,
}

/// One instance of a loaded struct (one branding of it), or a loaded enum. Interned per
/// `(node id, bindings)`, so the address is the identity.
pub(crate) struct LoadedBrand {
    node: &'static LoadedNode,
    /// The bindings of `node.scopes`' parameters, flattened in order. Unbound is `AnyPointer`.
    bindings: Box<[Type]>,
    /// By field index. Set once, before the instance is published.
    fields: OnceLock<Box<[LazyType]>>,
    /// Set once, before the instance is published.
    annotations: OnceLock<AnnotationTypes>,
}

impl LoadedBrand {
    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            scopes: self.node.scopes,
            bindings: &self.bindings,
        }
    }

    fn raw_struct(&'static self) -> RawBrandedStructSchema {
        RawBrandedStructSchema {
            generic: self.node.raw.expect("a loaded struct"),
            brand: StructBrand::Loaded(BrandRef::new(self)),
        }
    }

    fn raw_enum(&'static self) -> RawEnumSchema {
        RawEnumSchema {
            arena: self.node.arena,
            annotations: AnnotationSource::Loaded(BrandRef::new(self)),
        }
    }

    fn struct_type(&'static self) -> Type {
        TypeVariant::Struct(self.raw_struct()).into()
    }

    fn get(&'static self, lazy: &LazyType, fill: impl FnOnce(&mut Inner) -> Type) -> Type {
        match lazy {
            LazyType::Resolved(t) => *t,
            LazyType::Deferred(cell) => *cell.get_or_init(|| fill(&mut lock(self.node.loader))),
        }
    }

    pub(crate) fn field_type(&'static self, index: u16) -> Type {
        let fields = self.fields.get().expect("loaded tables are filled at load");
        let Some(lazy) = fields.get(usize::from(index)) else {
            introspect::panic_invalid_field_index(index)
        };
        self.get(lazy, |inner| {
            inner
                .resolve_field(self, index, true)
                .ok()
                .flatten()
                .unwrap_or_else(any_pointer)
        })
    }

    pub(crate) fn annotation_type(&'static self, child: Option<u16>, index: u32) -> Type {
        let annotations = self
            .annotations
            .get()
            .expect("loaded tables are filled at load");
        let list = match child {
            None => Some(&annotations.node),
            Some(c) => annotations.children.get(usize::from(c)),
        };
        let Some(lazy) = list.and_then(|l| l.get(index as usize)) else {
            introspect::panic_invalid_annotation_indices(child, index)
        };
        self.get(lazy, |inner| {
            inner
                .resolve_annotation(self, child, index, true)
                .ok()
                .flatten()
                .unwrap_or_else(any_pointer)
        })
    }

    pub(crate) fn brand_binding(&self, scope_id: u64, index: u16) -> Option<Type> {
        self.ctx().get(scope_id, index)
    }
}

/// How a [`Type`] refers to a loaded instance: a pointer to a `LoadedBrand` that lives forever.
///
/// A pointer rather than a `&'static`, only so that clippy's `mutable_key_type` lint doesn't see
/// the instance's write-once caches through it and warn on every `HashMap<Field, _>`. Nothing
/// observable about a `Type` ever changes.
#[derive(Clone, Copy)]
pub(crate) struct BrandRef(core::ptr::NonNull<LoadedBrand>);

// SAFETY: a `BrandRef` is only made from a `&'static LoadedBrand` (one that is never freed), so
// it is as `Send` and `Sync` as that reference, which they are because `LoadedBrand: Sync`.
unsafe impl Send for BrandRef {}
unsafe impl Sync for BrandRef {}

const _: () = {
    const fn assert_sync<T: Sync>() {}
    assert_sync::<LoadedBrand>()
};

impl BrandRef {
    fn new(brand: &'static LoadedBrand) -> Self {
        Self(core::ptr::NonNull::from(brand))
    }

    pub(crate) fn get(self) -> &'static LoadedBrand {
        // SAFETY: made from a `&'static LoadedBrand` in `new`.
        unsafe { self.0.as_ref() }
    }

    /// The identity of the instance.
    pub(crate) fn addr(self) -> *const LoadedBrand {
        self.0.as_ptr()
    }
}

/// Bindings in effect while resolving a type: generic parameter `(scope id, index)` maps to
/// `bindings[offset of the scope + index]`.
#[derive(Clone, Copy)]
struct Ctx<'c> {
    scopes: &'c [BrandScope],
    bindings: &'c [Type],
}

impl Ctx<'_> {
    fn get(&self, scope_id: u64, index: u16) -> Option<Type> {
        let mut offset = 0;
        for s in self.scopes {
            if s.scope_id() == scope_id {
                if index < s.param_count() {
                    return self.bindings.get(offset + usize::from(index)).copied();
                }
                return None;
            }
            offset += usize::from(s.param_count());
        }
        None
    }
}

#[derive(Default)]
struct Inner {
    nodes: HashMap<u64, &'static LoadedNode>,
    /// The default (all-`AnyPointer`) instance of every loaded struct and enum.
    defaults: HashMap<u64, &'static LoadedBrand>,
    /// Every other struct instance.
    instances: HashMap<(u64, Box<[Type]>), &'static LoadedBrand>,
}

/// Node lookup for validation: the batch being loaded plus everything already loaded.
trait Graph<'a> {
    fn node(&self, id: u64) -> Option<node::Reader<'a>>;
    /// See [`LoadedNode::parent`].
    fn parent(&self, id: u64) -> u64;
}

impl Graph<'static> for Inner {
    fn node(&self, id: u64) -> Option<node::Reader<'static>> {
        self.nodes.get(&id).map(|n| n.proto())
    }
    fn parent(&self, id: u64) -> u64 {
        self.nodes.get(&id).map_or(0, |n| n.parent)
    }
}

struct Batch<'a> {
    nodes: HashMap<u64, node::Reader<'a>>,
    parents: HashMap<u64, u64>,
}

impl<'a> Graph<'a> for Batch<'a> {
    fn node(&self, id: u64) -> Option<node::Reader<'a>> {
        self.nodes.get(&id).copied()
    }
    fn parent(&self, id: u64) -> u64 {
        self.parents.get(&id).copied().unwrap_or(0)
    }
}

impl<'a> NodeLookup<'a> for Batch<'a> {
    fn node(&self, id: u64) -> Option<node::Reader<'a>> {
        self.nodes.get(&id).copied()
    }
}

/// What load computes for a new node before anything is committed.
struct Prepared {
    id: u64,
    kind: Kind,
    parent: u64,
    scopes: Vec<BrandScope>,
    tables: Option<StructTables>,
}

struct StructTables {
    nonunion_members: Vec<u16>,
    members_by_discriminant: Vec<u16>,
    members_by_name: Vec<u16>,
    field_newtypes: Option<Vec<Vec<u64>>>,
    layout: Option<UseSiteLayout>,
}

impl Inner {
    fn loaded(&self, id: u64) -> Result<&'static LoadedNode> {
        self.nodes.get(&id).copied().ok_or_else(|| missing(id))
    }

    fn default_brand(&self, id: u64) -> Result<&'static LoadedBrand> {
        match self.defaults.get(&id) {
            Some(b) => Ok(b),
            None => {
                let node = self.loaded(id)?;
                Err(wrong_kind(
                    id,
                    if node.kind == Kind::Enum {
                        "an enum"
                    } else {
                        "a struct or enum"
                    },
                ))
            }
        }
    }

    fn load_batch(
        &mut self,
        me: &'static Mutex<Inner>,
        copies: Vec<(u64, Vec<Word>)>,
    ) -> Result<()> {
        // Content dedup: identical reloads are no-ops, anything else with a known id is a
        // conflict.
        let mut new: Vec<(u64, Vec<Word>)> = Vec::new();
        let mut seen: HashMap<u64, usize> = HashMap::new();
        for (id, words) in copies {
            if let Some(existing) = self.nodes.get(&id) {
                if existing.words == &words[..] {
                    continue;
                }
                return Err(failed(format!(
                    "schema loader: node {id:#x} is already loaded with different content"
                )));
            }
            if let Some(&i) = seen.get(&id) {
                if new[i].1 == words {
                    continue;
                }
                return Err(failed(format!(
                    "schema loader: the batch contains node {id:#x} twice, with different content"
                )));
            }
            seen.insert(id, new.len());
            new.push((id, words));
        }
        if new.is_empty() {
            return Ok(());
        }

        let prepared = self.prepare(&new)?;

        // Commit. Nothing below fails: everything was validated and computed above.
        let mut added = Vec::new();
        for (p, (id, words)) in prepared.into_iter().zip(new) {
            debug_assert_eq!(p.id, id);
            let words: &'static [Word] = leak_slice(words);
            let arena: &'static GeneratedCodeArena =
                Box::leak(Box::new(GeneratedCodeArena::new(words)));
            let scopes = leak_slice(p.scopes);
            let raw = p.tables.map(|t| {
                let mut raw = RawStructSchema::new(
                    arena,
                    leak_slice(t.nonunion_members),
                    leak_slice(t.members_by_discriminant),
                    leak_slice(t.members_by_name),
                )
                .with_brand_scopes(scopes);
                if let Some(f) = t.field_newtypes {
                    raw = raw
                        .with_field_newtypes(leak_slice(f.into_iter().map(leak_slice).collect()));
                }
                if let Some(layout) = t.layout {
                    raw = raw.with_any_layout(leak_slice(layout.newtype_ids), layout.table.leak());
                }
                &*Box::leak(Box::new(raw))
            });
            let node: &'static LoadedNode = Box::leak(Box::new(LoadedNode {
                words,
                arena,
                kind: p.kind,
                parent: p.parent,
                scopes,
                raw,
                loader: me,
            }));
            self.nodes.insert(id, node);
            added.push(node);
        }

        // Phase 1: every new struct and enum gets its default instance, so that the tables
        // filled in phase 2 can point at any of them, including recursively.
        let mut defaults = Vec::new();
        for node in added {
            if node.kind == Kind::Struct || node.kind == Kind::Enum {
                let brand: &'static LoadedBrand = Box::leak(Box::new(LoadedBrand {
                    node,
                    bindings: vec![any_pointer(); node.param_count()].into_boxed_slice(),
                    fields: OnceLock::new(),
                    annotations: OnceLock::new(),
                }));
                self.defaults.insert(node.proto().get_id(), brand);
                defaults.push(brand);
            }
        }
        // Phase 2: fill their tables.
        for brand in defaults {
            self.fill(brand);
        }
        Ok(())
    }

    /// Validates the new nodes and computes what they need, without committing anything.
    fn prepare(&self, new: &[(u64, Vec<Word>)]) -> Result<Vec<Prepared>> {
        let segments: Vec<Vec<&[u8]>> = new
            .iter()
            .map(|(_, words)| vec![Word::words_to_bytes(words)])
            .collect();
        let mut options = message::ReaderOptions::new();
        options.traversal_limit_in_words(None);
        let messages: Vec<message::Reader<&Vec<&[u8]>>> = segments
            .iter()
            .map(|s| message::Reader::new(s, options))
            .collect();
        let mut readers = Vec::new();
        for m in &messages {
            readers.push(m.get_root::<node::Reader<'_>>()?);
        }

        let mut batch = Batch {
            nodes: HashMap::new(),
            parents: HashMap::new(),
        };
        for (id, node) in &self.nodes {
            batch.nodes.insert(*id, node.proto());
            batch.parents.insert(*id, node.parent);
        }
        for r in &readers {
            batch.nodes.insert(r.get_id(), *r);
            batch.parents.insert(r.get_id(), r.get_scope_id());
        }
        // A method's implicit parameter and result structs have no scope; their parent is the
        // interface (as in capnpc's codegen).
        for r in &readers {
            if let Ok(node::Interface(interface)) = r.which() {
                for method in interface.get_methods()? {
                    for id in [
                        method.get_param_struct_type(),
                        method.get_result_struct_type(),
                    ] {
                        if batch.parents.get(&id) == Some(&0) && !self.nodes.contains_key(&id) {
                            batch.parents.insert(id, r.get_id());
                        }
                    }
                }
            }
        }

        let mut prepared = Vec::new();
        for r in &readers {
            prepared.push(prepare_node(&batch, *r).map_err(|e| {
                let name = r
                    .get_display_name()
                    .ok()
                    .and_then(|n| n.to_str().ok())
                    .unwrap_or("?");
                let mut e = e;
                e.extra = format!("{} (in node {:#x} `{name}`)", e.extra, r.get_id());
                e
            })?);
        }
        Ok(prepared)
    }

    /// Fills `brand`'s tables. Types naming instantiations not interned yet are deferred.
    fn fill(&mut self, brand: &'static LoadedBrand) {
        fn lazy(r: Result<Option<Type>>) -> LazyType {
            match r {
                Ok(Some(t)) => LazyType::Resolved(t),
                Ok(None) => LazyType::Deferred(OnceLock::new()),
                Err(e) => {
                    // Load validated everything resolution depends on.
                    debug_assert!(
                        false,
                        "schema loader: validated node failed to resolve: {e}"
                    );
                    LazyType::Resolved(any_pointer())
                }
            }
        }

        let proto = brand.node.proto();
        let count = |l: Result<crate::struct_list::Reader<'static, annotation::Owned>>| {
            l.map_or(0, |l| l.len())
        };
        let mut fields = Vec::new();
        let mut child_counts = Vec::new();
        match proto.which() {
            Ok(node::Struct(st)) => {
                if let Ok(fs) = st.get_fields() {
                    for (i, f) in fs.iter().enumerate() {
                        let i = u16::try_from(i).expect("validated field count");
                        fields.push(lazy(self.resolve_field(brand, i, false)));
                        child_counts.push(count(f.get_annotations()));
                    }
                }
            }
            Ok(node::Enum(en)) => {
                if let Ok(es) = en.get_enumerants() {
                    for e in es {
                        child_counts.push(count(e.get_annotations()));
                    }
                }
            }
            _ => {}
        }
        let own = (0..count(proto.get_annotations()))
            .map(|i| lazy(self.resolve_annotation(brand, None, i, false)))
            .collect();
        let children = child_counts
            .into_iter()
            .enumerate()
            .map(|(c, n)| {
                let c = u16::try_from(c).expect("validated member count");
                (0..n)
                    .map(|i| lazy(self.resolve_annotation(brand, Some(c), i, false)))
                    .collect()
            })
            .collect();
        let _ = brand.fields.set(fields.into_boxed_slice());
        let _ = brand.annotations.set(AnnotationTypes {
            node: own,
            children,
        });
    }

    /// The instance of struct `id` with `bindings`. `None` if it isn't interned yet and
    /// `intern` is false.
    fn instance(
        &mut self,
        id: u64,
        bindings: Box<[Type]>,
        intern: bool,
    ) -> Result<Option<&'static LoadedBrand>> {
        let default = self.default_brand(id)?;
        if default.node.raw.is_none() {
            return Err(wrong_kind(id, "a struct"));
        }
        if bindings.iter().all(|t| *t == any_pointer()) {
            return Ok(Some(default));
        }
        let key = (id, bindings);
        if let Some(b) = self.instances.get(&key) {
            return Ok(Some(b));
        }
        if !intern {
            return Ok(None);
        }
        let brand: &'static LoadedBrand = Box::leak(Box::new(LoadedBrand {
            node: default.node,
            bindings: key.1.clone(),
            fields: OnceLock::new(),
            annotations: OnceLock::new(),
        }));
        // Insert first, so a field naming this same instance finds it.
        self.instances.insert(key, brand);
        self.fill(brand);
        Ok(Some(brand))
    }

    /// The bindings of struct (or annotation) `target`'s scopes that `brand` gives, in `ctx`.
    fn resolve_brand(
        &mut self,
        target: u64,
        brand: brand::Reader<'_>,
        ctx: Ctx<'_>,
        intern: bool,
    ) -> Result<Option<Box<[Type]>>> {
        let node = self.loaded(target)?;
        let scopes = brand.get_scopes()?;
        let mut out = Vec::with_capacity(node.param_count());
        for target_scope in node.scopes {
            let sid = target_scope.scope_id();
            let count = target_scope.param_count();
            match scopes.iter().find(|s| s.get_scope_id() == sid) {
                None => out.extend((0..count).map(|_| any_pointer())),
                Some(s) => match s.which()? {
                    brand::scope::Inherit(()) => {
                        out.extend((0..count).map(|i| ctx.get(sid, i).unwrap_or_else(any_pointer)))
                    }
                    brand::scope::Bind(list) => {
                        let list = list?;
                        for i in 0..count {
                            let t = if u32::from(i) < list.len() {
                                match list.get(u32::from(i)).which()? {
                                    brand::binding::Unbound(()) => any_pointer(),
                                    brand::binding::Type(t) => {
                                        match self.resolve_type(t?, ctx, intern)? {
                                            Some(t) => t,
                                            None => return Ok(None),
                                        }
                                    }
                                }
                            } else {
                                any_pointer()
                            };
                            out.push(t);
                        }
                    }
                },
            }
        }
        Ok(Some(out.into_boxed_slice()))
    }

    /// Resolves `ty` in `ctx`. `None` if it names a struct instantiation that isn't interned
    /// yet and `intern` is false.
    fn resolve_type(
        &mut self,
        ty: type_::Reader<'_>,
        ctx: Ctx<'_>,
        intern: bool,
    ) -> Result<Option<Type>> {
        Ok(Some(match ty.which()? {
            type_::Void(()) => <()>::introspect(),
            type_::Bool(()) => bool::introspect(),
            type_::Int8(()) => i8::introspect(),
            type_::Int16(()) => i16::introspect(),
            type_::Int32(()) => i32::introspect(),
            type_::Int64(()) => i64::introspect(),
            type_::Uint8(()) => u8::introspect(),
            type_::Uint16(()) => u16::introspect(),
            type_::Uint32(()) => u32::introspect(),
            type_::Uint64(()) => u64::introspect(),
            type_::Float32(()) => f32::introspect(),
            type_::Float64(()) => f64::introspect(),
            type_::Text(()) => crate::text::Owned::introspect(),
            type_::Data(()) => crate::data::Owned::introspect(),
            type_::List(l) => match self.resolve_type(l.get_element_type()?, ctx, intern)? {
                Some(t) => Type::list_of(t),
                None => return Ok(None),
            },
            type_::Enum(e) => {
                let id = e.get_type_id();
                let brand = self.default_brand(id)?;
                if brand.node.kind != Kind::Enum {
                    return Err(wrong_kind(id, "an enum"));
                }
                TypeVariant::Enum(brand.raw_enum()).into()
            }
            type_::Struct(s) => {
                let id = s.get_type_id();
                let Some(bindings) = self.resolve_brand(id, s.get_brand()?, ctx, intern)? else {
                    return Ok(None);
                };
                match self.instance(id, bindings, intern)? {
                    Some(b) => b.struct_type(),
                    None => return Ok(None),
                }
            }
            type_::Interface(_) => TypeVariant::Capability.into(),
            type_::AnyPointer(a) => match a.which()? {
                type_::any_pointer::Unconstrained(_) => any_pointer(),
                type_::any_pointer::Parameter(p) => ctx
                    .get(p.get_scope_id(), p.get_parameter_index())
                    .unwrap_or_else(any_pointer),
                type_::any_pointer::ImplicitMethodParameter(_) => any_pointer(),
            },
        }))
    }

    /// The type of field `index` of `brand`.
    fn resolve_field(
        &mut self,
        brand: &'static LoadedBrand,
        index: u16,
        intern: bool,
    ) -> Result<Option<Type>> {
        let node::Struct(st) = brand.node.proto().which()? else {
            return Err(failed("schema loader: not a struct".into()));
        };
        let field = st.get_fields()?.get(u32::from(index));
        match field.which()? {
            field::Slot(slot) => self.resolve_type(slot.get_type()?, brand.ctx(), intern),
            field::Group(group) => {
                // A group shares its parent's scopes, so it takes the parent's bindings.
                let id = group.get_type_id();
                let group_node = self.loaded(id)?;
                let ctx = brand.ctx();
                let bindings = group_node
                    .scopes
                    .iter()
                    .flat_map(|s| {
                        (0..s.param_count())
                            .map(move |i| ctx.get(s.scope_id(), i).unwrap_or_else(any_pointer))
                    })
                    .collect();
                Ok(self
                    .instance(id, bindings, intern)?
                    .map(|b| b.struct_type()))
            }
        }
    }

    /// The type of the value of annotation `index` on `brand`'s node (`child == None`), or on
    /// its field or enumerant `child`.
    fn resolve_annotation(
        &mut self,
        brand: &'static LoadedBrand,
        child: Option<u16>,
        index: u32,
        intern: bool,
    ) -> Result<Option<Type>> {
        let proto = brand.node.proto();
        let annotations = match child {
            None => proto.get_annotations()?,
            Some(c) => match proto.which()? {
                node::Struct(st) => st.get_fields()?.get(u32::from(c)).get_annotations()?,
                node::Enum(en) => en.get_enumerants()?.get(u32::from(c)).get_annotations()?,
                _ => return Err(failed("schema loader: no such annotation".into())),
            },
        };
        let annotation = annotations.get(index);
        let id = annotation.get_id();
        let node::Annotation(decl) = self.loaded(id)?.proto().which()? else {
            return Err(wrong_kind(id, "an annotation"));
        };
        let Some(bindings) =
            self.resolve_brand(id, annotation.get_brand()?, brand.ctx(), intern)?
        else {
            return Ok(None);
        };
        let ctx = Ctx {
            scopes: self.loaded(id)?.scopes,
            bindings: &bindings,
        };
        self.resolve_type(decl.get_type()?, ctx, intern)
    }
}

// ---------------------------------------------------------------------------------------------
// Validation

fn expect_kind<'a>(graph: &impl Graph<'a>, id: u64, kind: Kind) -> Result<node::Reader<'a>> {
    let Some(node) = graph.node(id) else {
        return Err(failed(format!(
            "schema loader: references node {id:#x}, which is not loaded (expected {})",
            kind.name()
        )));
    };
    let actual = Kind::of(node)?;
    if actual != kind {
        return Err(failed(format!(
            "schema loader: references node {id:#x} as {}, but it is {}",
            kind.name(),
            actual.name()
        )));
    }
    Ok(node)
}

/// The node's ancestors (itself first), following parents. Fails on a cycle, and on a missing
/// ancestor if `complete`.
fn ancestors<'a>(graph: &impl Graph<'a>, id: u64, complete: bool) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    let mut cur = id;
    while cur != 0 {
        if out.contains(&cur) {
            return Err(failed(format!(
                "schema loader: the scope chain of node {id:#x} has a cycle"
            )));
        }
        if graph.node(cur).is_none() {
            if complete {
                return Err(failed(format!(
                    "schema loader: scope {cur:#x} of node {id:#x} is not loaded"
                )));
            }
            break;
        }
        out.push(cur);
        cur = graph.parent(cur);
    }
    Ok(out)
}

/// The brand scopes of node `id`, as capnpc's codegen computes them: none for a node that
/// isn't generic, otherwise every scope in its chain that declares parameters, outermost first.
fn brand_scopes<'a>(graph: &impl Graph<'a>, node: node::Reader<'a>) -> Result<Vec<BrandScope>> {
    if !node.get_is_generic() {
        return Ok(Vec::new());
    }
    let mut scopes = Vec::new();
    for id in ancestors(graph, node.get_id(), true)? {
        let n = graph.node(id).expect("ancestor");
        let count = n.get_parameters()?.len();
        if count > 0 {
            let count = u16::try_from(count)
                .map_err(|_| failed("schema loader: too many generic parameters".into()))?;
            scopes.push(BrandScope::new(id, count));
        }
    }
    scopes.reverse();
    Ok(scopes)
}

fn is_pointer_type(ty: type_::Reader<'_>) -> Result<bool> {
    Ok(matches!(
        ty.which()?,
        type_::Text(())
            | type_::Data(())
            | type_::List(_)
            | type_::Struct(_)
            | type_::Interface(_)
            | type_::AnyPointer(_)
    ))
}

/// Checks a `Type`: every node it names exists with the right kind, brand bindings are pointer
/// types and, if `strict`, generic parameter references and brand scopes are in scope. `scopes`
/// are the brand scopes of the node the type appears in.
fn check_type<'a>(
    graph: &impl Graph<'a>,
    ty: type_::Reader<'_>,
    scopes: &[BrandScope],
    strict: bool,
    depth: u32,
) -> Result<()> {
    if depth > MAX_TYPE_DEPTH {
        return Err(failed("schema loader: type nests too deeply".into()));
    }
    match ty.which()? {
        type_::Void(())
        | type_::Bool(())
        | type_::Int8(())
        | type_::Int16(())
        | type_::Int32(())
        | type_::Int64(())
        | type_::Uint8(())
        | type_::Uint16(())
        | type_::Uint32(())
        | type_::Uint64(())
        | type_::Float32(())
        | type_::Float64(())
        | type_::Text(())
        | type_::Data(()) => {}
        type_::List(l) => check_type(graph, l.get_element_type()?, scopes, strict, depth + 1)?,
        type_::Enum(e) => {
            expect_kind(graph, e.get_type_id(), Kind::Enum)?;
            check_brand(
                graph,
                e.get_brand()?,
                e.get_type_id(),
                scopes,
                strict,
                depth + 1,
            )?;
        }
        type_::Struct(s) => {
            expect_kind(graph, s.get_type_id(), Kind::Struct)?;
            check_brand(
                graph,
                s.get_brand()?,
                s.get_type_id(),
                scopes,
                strict,
                depth + 1,
            )?;
        }
        type_::Interface(i) => {
            expect_kind(graph, i.get_type_id(), Kind::Interface)?;
            check_brand(
                graph,
                i.get_brand()?,
                i.get_type_id(),
                scopes,
                strict,
                depth + 1,
            )?;
        }
        type_::AnyPointer(a) => match a.which()? {
            type_::any_pointer::Unconstrained(u) => {
                u.which()?;
            }
            type_::any_pointer::Parameter(p) => {
                let sid = p.get_scope_id();
                let index = p.get_parameter_index();
                if strict
                    && !scopes
                        .iter()
                        .any(|s| s.scope_id() == sid && index < s.param_count())
                {
                    return Err(failed(format!(
                        "schema loader: generic parameter {index} of scope {sid:#x} is not in scope"
                    )));
                }
            }
            type_::any_pointer::ImplicitMethodParameter(_) => {}
        },
    }
    let newtype = ty.get_type_id();
    if newtype != 0 {
        expect_kind(graph, newtype, Kind::Type)?;
    }
    Ok(())
}

/// Checks the brand of a reference to node `target`; see [`check_type`].
fn check_brand<'a>(
    graph: &impl Graph<'a>,
    brand: brand::Reader<'_>,
    target: u64,
    scopes: &[BrandScope],
    strict: bool,
    depth: u32,
) -> Result<()> {
    if depth > MAX_TYPE_DEPTH {
        return Err(failed("schema loader: type nests too deeply".into()));
    }
    let brand_scopes = brand.get_scopes()?;
    if brand_scopes.is_empty() {
        return Ok(());
    }
    let target_ancestors = if strict {
        ancestors(graph, target, true)?
    } else {
        Vec::new()
    };
    for scope in brand_scopes {
        let sid = scope.get_scope_id();
        if strict && !target_ancestors.contains(&sid) {
            return Err(failed(format!(
                "schema loader: a brand of node {target:#x} binds scope {sid:#x}, which is not one of its scopes"
            )));
        }
        match scope.which()? {
            brand::scope::Inherit(()) => {}
            brand::scope::Bind(bindings) => {
                let bindings = bindings?;
                if strict {
                    let count = graph.node(sid).expect("ancestor").get_parameters()?.len();
                    if bindings.len() != count {
                        return Err(failed(format!(
                            "schema loader: a brand binds {} parameters of scope {sid:#x}, which has {count}",
                            bindings.len()
                        )));
                    }
                }
                for binding in bindings {
                    match binding.which()? {
                        brand::binding::Unbound(()) => {}
                        brand::binding::Type(t) => {
                            let t = t?;
                            check_type(graph, t, scopes, strict, depth + 1)?;
                            if !is_pointer_type(t)? {
                                return Err(failed(
                                    "schema loader: a generic parameter is bound to a non-pointer type"
                                        .into(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_annotations<'a>(
    graph: &impl Graph<'a>,
    annotations: crate::struct_list::Reader<'_, annotation::Owned>,
    scopes: &[BrandScope],
) -> Result<()> {
    for a in annotations {
        expect_kind(graph, a.get_id(), Kind::Annotation)?;
        check_brand(graph, a.get_brand()?, a.get_id(), scopes, true, 0)?;
    }
    Ok(())
}

/// The data bits and pointer slots a value of `ty` occupies, and checks that `value` is of the
/// matching union case (`dynamic_struct` matches on the pair).
fn check_value(ty: type_::Reader<'_>, value: value::Reader<'_>) -> Result<(u64, u64)> {
    let ok = match (ty.which()?, value.which()?) {
        (type_::Void(()), value::Void(())) => Some((0, 0)),
        (type_::Bool(()), value::Bool(_)) => Some((1, 0)),
        (type_::Int8(()), value::Int8(_)) | (type_::Uint8(()), value::Uint8(_)) => Some((8, 0)),
        (type_::Int16(()), value::Int16(_))
        | (type_::Uint16(()), value::Uint16(_))
        | (type_::Enum(_), value::Enum(_)) => Some((16, 0)),
        (type_::Int32(()), value::Int32(_))
        | (type_::Uint32(()), value::Uint32(_))
        | (type_::Float32(()), value::Float32(_)) => Some((32, 0)),
        (type_::Int64(()), value::Int64(_))
        | (type_::Uint64(()), value::Uint64(_))
        | (type_::Float64(()), value::Float64(_)) => Some((64, 0)),
        (type_::Text(()), value::Text(_))
        | (type_::Data(()), value::Data(_))
        | (type_::List(_), value::List(_))
        | (type_::Struct(_), value::Struct(_))
        | (type_::Interface(_), value::Interface(()))
        | (type_::AnyPointer(_), value::AnyPointer(_)) => Some((0, 1)),
        _ => None,
    };
    ok.ok_or_else(|| failed("schema loader: a default value does not match its type".into()))
}

fn check_member_order<'a>(
    names: &mut HashSet<&'a str>,
    code_orders: &mut [bool],
    name: &'a str,
    code_order: u16,
) -> Result<()> {
    if !names.insert(name) {
        return Err(failed(format!(
            "schema loader: duplicate member name `{name}`"
        )));
    }
    match code_orders.get_mut(usize::from(code_order)) {
        Some(seen @ false) => *seen = true,
        _ => {
            return Err(failed(format!(
                "schema loader: member `{name}` has an invalid codeOrder"
            )))
        }
    }
    Ok(())
}

/// Validates a new node and computes its tables.
fn prepare_node<'a>(graph: &Batch<'a>, node: node::Reader<'a>) -> Result<Prepared> {
    let id = node.get_id();
    let kind = Kind::of(node)?;
    if !node.get_parameters()?.is_empty() && !node.get_is_generic() {
        return Err(failed(
            "schema loader: a node with generic parameters must be marked isGeneric".into(),
        ));
    }
    // Scope chains must not loop (groups are nested through them).
    ancestors(graph, id, false)?;
    let scopes = brand_scopes(graph, node)?;
    check_annotations(graph, node.get_annotations()?, &scopes)?;

    let mut tables = None;
    match node.which()? {
        node::File(()) => {}
        node::Struct(st) => tables = Some(check_struct(graph, node, st, &scopes)?),
        node::Enum(en) => {
            let enumerants = en.get_enumerants()?;
            if enumerants.len() >= u32::from(u16::MAX) {
                return Err(failed("schema loader: too many enumerants".into()));
            }
            let mut names = HashSet::new();
            let mut code_orders = vec![false; enumerants.len() as usize];
            for e in enumerants {
                check_member_order(
                    &mut names,
                    &mut code_orders,
                    e.get_name()?.to_str()?,
                    e.get_code_order(),
                )?;
                check_annotations(graph, e.get_annotations()?, &scopes)?;
            }
        }
        node::Interface(interface) => {
            for superclass in interface.get_superclasses()? {
                expect_kind(graph, superclass.get_id(), Kind::Interface)?;
                check_brand(
                    graph,
                    superclass.get_brand()?,
                    superclass.get_id(),
                    &scopes,
                    true,
                    0,
                )?;
            }
            let methods = interface.get_methods()?;
            let mut names = HashSet::new();
            let mut code_orders = vec![false; methods.len() as usize];
            for m in methods {
                check_member_order(
                    &mut names,
                    &mut code_orders,
                    m.get_name()?.to_str()?,
                    m.get_code_order(),
                )?;
                expect_kind(graph, m.get_param_struct_type(), Kind::Struct)?;
                check_brand(
                    graph,
                    m.get_param_brand()?,
                    m.get_param_struct_type(),
                    &scopes,
                    true,
                    0,
                )?;
                expect_kind(graph, m.get_result_struct_type(), Kind::Struct)?;
                check_brand(
                    graph,
                    m.get_result_brand()?,
                    m.get_result_struct_type(),
                    &scopes,
                    true,
                    0,
                )?;
                check_annotations(graph, m.get_annotations()?, &scopes)?;
            }
        }
        node::Const(c) => {
            let ty = c.get_type()?;
            check_type(graph, ty, &scopes, true, 0)?;
            check_value(ty, c.get_value()?)?;
        }
        node::Annotation(a) => check_type(graph, a.get_type()?, &scopes, true, 0)?,
        node::Type(t) => {
            let t = t?;
            check_type(graph, t, &scopes, true, 0)?;
            // Alias chains (`type B = A`) must end.
            let mut seen = vec![id];
            let mut alias = t.get_type_id();
            while alias != 0 {
                if seen.contains(&alias) {
                    return Err(failed(format!(
                        "schema loader: newtype {id:#x} is part of an alias cycle"
                    )));
                }
                seen.push(alias);
                let node::Type(aliased) = expect_kind(graph, alias, Kind::Type)?.which()? else {
                    unreachable!()
                };
                alias = aliased?.get_type_id();
            }
        }
    }
    Ok(Prepared {
        id,
        kind,
        parent: graph.parent(id),
        scopes,
        tables,
    })
}

fn check_struct<'a>(
    graph: &Batch<'a>,
    node: node::Reader<'a>,
    st: node::struct_::Reader<'a>,
    scopes: &[BrandScope],
) -> Result<StructTables> {
    let id = node.get_id();
    let data_bits = u64::from(st.get_data_word_count()) * 64;
    let pointers = u64::from(st.get_pointer_count());
    let fields = st.get_fields()?;
    let field_count = fields.len();
    if field_count >= u32::from(u16::MAX) {
        return Err(failed("schema loader: too many fields".into()));
    }
    let discriminant_count = st.get_discriminant_count();
    if discriminant_count > 0 {
        if discriminant_count == 1 {
            return Err(failed(
                "schema loader: a union must have at least two members".into(),
            ));
        }
        if u32::from(discriminant_count) > field_count {
            return Err(failed(
                "schema loader: more union members than fields".into(),
            ));
        }
        if (u64::from(st.get_discriminant_offset()) + 1) * 16 > data_bits {
            return Err(failed(
                "schema loader: the union discriminant is out of bounds".into(),
            ));
        }
    }

    let mut names = HashSet::new();
    let mut code_orders = vec![false; field_count as usize];
    let mut seen_discriminants = vec![false; usize::from(discriminant_count)];
    let mut nonunion_members = Vec::new();
    let mut union_members = Vec::new();
    let mut by_name = Vec::new();
    for (index, f) in fields.iter().enumerate() {
        let index = u16::try_from(index).expect("field count checked above");
        let name = f.get_name()?.to_str()?;
        check_member_order(&mut names, &mut code_orders, name, f.get_code_order())?;
        f.get_ordinal().which()?;
        by_name.push((name, index));

        let discriminant = f.get_discriminant_value();
        if discriminant == field::NO_DISCRIMINANT {
            nonunion_members.push(index);
        } else {
            match seen_discriminants.get_mut(usize::from(discriminant)) {
                Some(seen @ false) => *seen = true,
                _ => {
                    return Err(failed(format!(
                        "schema loader: field `{name}` has an invalid or duplicate discriminant {discriminant}"
                    )))
                }
            }
            union_members.push((discriminant, index));
        }

        check_annotations(graph, f.get_annotations()?, scopes)?;
        if f.get_type_id() != 0 {
            expect_kind(graph, f.get_type_id(), Kind::Type)?;
        }
        match f.which()? {
            field::Slot(slot) => {
                let ty = slot.get_type()?;
                check_type(graph, ty, scopes, true, 0)?;
                let (bits, ptrs) = check_value(ty, slot.get_default_value()?)?;
                let offset = u64::from(slot.get_offset());
                if bits * (offset + 1) > data_bits || ptrs * (offset + 1) > pointers {
                    return Err(failed(format!(
                        "schema loader: field `{name}`'s offset is out of bounds"
                    )));
                }
            }
            field::Group(group) => {
                let group_node = expect_kind(graph, group.get_type_id(), Kind::Struct)?;
                let node::Struct(gst) = group_node.which()? else {
                    unreachable!()
                };
                if !gst.get_is_group() || group_node.get_scope_id() != id {
                    return Err(failed(format!(
                        "schema loader: group field `{name}` names node {:#x}, which is not a group of this struct",
                        group.get_type_id()
                    )));
                }
                // The group's members are checked against its own sizes; they must fit this
                // struct, through which the group is read and written.
                if gst.get_data_word_count() > st.get_data_word_count()
                    || gst.get_pointer_count() > st.get_pointer_count()
                {
                    return Err(failed(format!(
                        "schema loader: group field `{name}` is larger than its parent"
                    )));
                }
            }
        }
    }
    if union_members.len() != usize::from(discriminant_count) {
        return Err(failed(
            "schema loader: discriminantCount does not match the union's members".into(),
        ));
    }
    if st.get_is_group() {
        if node.get_scope_id() == 0 {
            return Err(failed("schema loader: a group must have a parent".into()));
        }
        expect_kind(graph, node.get_scope_id(), Kind::Struct)?;
    }

    // The tables, as capnpc's codegen computes them (`generate_members_by_*`).
    union_members.sort_unstable();
    by_name.sort_by_key(|(name, _)| *name);
    let mut field_newtypes = Vec::new();
    for f in fields {
        field_newtypes.push(newtype_layout::field_newtype_ids(graph, f)?);
    }
    let field_newtypes = if field_newtypes.iter().any(|ids| !ids.is_empty()) {
        Some(field_newtypes)
    } else {
        None
    };
    Ok(StructTables {
        nonunion_members,
        members_by_discriminant: union_members.into_iter().map(|(_, i)| i).collect(),
        members_by_name: by_name.into_iter().map(|(_, i)| i).collect(),
        field_newtypes,
        layout: newtype_layout::use_site_layout(graph, node)?,
    })
}

#[cfg(test)]
mod tests;
