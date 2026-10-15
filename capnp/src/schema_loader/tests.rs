//! Loader tests over hand-built nodes: the semantics of loading, and validation of malformed
//! input. Conformance with compiled schemas is tested in capnpc-test, over real
//! `CodeGeneratorRequest`s.

use super::*;
use crate::schema_capnp::{brand, field, node, type_};
use crate::struct_list;

const FILE: u64 = 0x1000;
/// `struct Point { x @0 :Int32; y @1 :Int32 = 7; label @2 :Text; next @3 :Point;
///   color @4 :Color; }` with `$ann("point")`.
const POINT: u64 = 0x1001;
/// `enum Color { red @0; green @1 $ann("g"); }`
const COLOR: u64 = 0x1002;
/// `struct Shape { union { circle @0 :Float64; square @1 :Float64; } info :group { a @2 :UInt8; } }`
const SHAPE: u64 = 0x1003;
const SHAPE_INFO: u64 = 0x1004;
/// `annotation ann :Text;`
const ANN: u64 = 0x1005;
/// `struct Box(T) { v @0 :T; }`
const BOX: u64 = 0x1006;
/// `struct Use { b @0 :Box(Text); p @1 :Box(Point); u @2 :Box; }`
const USE: u64 = 0x1007;

const NODE_COUNT: u32 = 8;
const FILE_IX: u32 = 0;
const POINT_IX: u32 = 1;
const COLOR_IX: u32 = 2;
const SHAPE_IX: u32 = 3;
const SHAPE_INFO_IX: u32 = 4;
const ANN_IX: u32 = 5;
const BOX_IX: u32 = 6;
const USE_IX: u32 = 7;

fn node_header(mut n: node::Builder<'_>, id: u64, scope: u64, name: &str) {
    n.set_id(id);
    n.set_scope_id(scope);
    n.set_display_name(name);
}

fn slot<'a>(
    mut f: field::Builder<'a>,
    name: &str,
    code_order: u16,
    offset: u32,
) -> field::slot::Builder<'a> {
    f.set_name(name);
    f.set_code_order(code_order);
    let mut s = f.init_slot();
    s.set_offset(offset);
    s
}

fn text_annotation(
    list: struct_list::Builder<'_, crate::schema_capnp::annotation::Owned>,
    text: &str,
) {
    let mut a = list.get(0);
    a.set_id(ANN);
    a.reborrow().init_value().set_text(text);
    a.init_brand();
}

/// Builds the valid batch described by the constants above.
fn build(nodes: &mut struct_list::Builder<'_, node::Owned>) {
    let mut file = nodes.reborrow().get(FILE_IX);
    node_header(file.reborrow(), FILE, 0, "test.capnp");
    file.set_file(());

    let mut point = nodes.reborrow().get(POINT_IX);
    node_header(point.reborrow(), POINT, FILE, "test.capnp:Point");
    text_annotation(point.reborrow().init_annotations(1), "point");
    let mut st = point.init_struct();
    st.set_data_word_count(2);
    st.set_pointer_count(2);
    let mut fields = st.init_fields(5);
    let mut s = slot(fields.reborrow().get(0), "x", 0, 0);
    s.reborrow().init_type().set_int32(());
    s.init_default_value().set_int32(0);
    let mut s = slot(fields.reborrow().get(1), "y", 1, 1);
    s.reborrow().init_type().set_int32(());
    s.init_default_value().set_int32(7);
    let mut s = slot(fields.reborrow().get(2), "label", 2, 0);
    s.reborrow().init_type().set_text(());
    s.init_default_value().set_text("");
    let mut s = slot(fields.reborrow().get(3), "next", 3, 1);
    let mut t = s.reborrow().init_type().init_struct();
    t.set_type_id(POINT);
    t.init_brand();
    s.init_default_value().init_struct();
    let mut s = slot(fields.reborrow().get(4), "color", 4, 4);
    s.reborrow().init_type().init_enum().set_type_id(COLOR);
    s.init_default_value().set_enum(0);
    for i in 0..5 {
        fields
            .reborrow()
            .get(i)
            .set_discriminant_value(field::NO_DISCRIMINANT);
    }

    let mut color = nodes.reborrow().get(COLOR_IX);
    node_header(color.reborrow(), COLOR, FILE, "test.capnp:Color");
    let mut enumerants = color.init_enum().init_enumerants(2);
    enumerants.reborrow().get(0).set_name("red");
    let mut green = enumerants.reborrow().get(1);
    green.set_name("green");
    green.set_code_order(1);
    text_annotation(green.init_annotations(1), "g");

    let mut shape = nodes.reborrow().get(SHAPE_IX);
    node_header(shape.reborrow(), SHAPE, FILE, "test.capnp:Shape");
    let mut st = shape.init_struct();
    st.set_data_word_count(2);
    st.set_discriminant_count(2);
    st.set_discriminant_offset(4); // bits 64..80
    let mut fields = st.init_fields(3);
    for (i, name) in [(0, "circle"), (1, "square")] {
        let mut f = fields.reborrow().get(i);
        f.set_discriminant_value(i as u16);
        let mut s = slot(f, name, i as u16, 0);
        s.reborrow().init_type().set_float64(());
        s.init_default_value().set_float64(0.0);
    }
    let mut info = fields.reborrow().get(2);
    info.set_name("info");
    info.set_code_order(2);
    info.set_discriminant_value(field::NO_DISCRIMINANT);
    info.init_group().set_type_id(SHAPE_INFO);

    let mut shape_info = nodes.reborrow().get(SHAPE_INFO_IX);
    node_header(
        shape_info.reborrow(),
        SHAPE_INFO,
        SHAPE,
        "test.capnp:Shape.info",
    );
    let mut st = shape_info.init_struct();
    st.set_is_group(true);
    st.set_data_word_count(2);
    let mut f = st.init_fields(1).get(0);
    f.set_discriminant_value(field::NO_DISCRIMINANT);
    let mut s = slot(f, "a", 0, 10); // byte 10, in the second word
    s.reborrow().init_type().set_uint8(());
    s.init_default_value().set_uint8(0);

    let mut ann = nodes.reborrow().get(ANN_IX);
    node_header(ann.reborrow(), ANN, FILE, "test.capnp:ann");
    let mut a = ann.init_annotation();
    a.reborrow().init_type().set_text(());
    a.set_targets_struct(true);

    let mut boxn = nodes.reborrow().get(BOX_IX);
    node_header(boxn.reborrow(), BOX, FILE, "test.capnp:Box");
    boxn.set_is_generic(true);
    boxn.reborrow().init_parameters(1).get(0).set_name("T");
    let mut st = boxn.init_struct();
    st.set_pointer_count(1);
    let mut f = st.init_fields(1).get(0);
    f.set_discriminant_value(field::NO_DISCRIMINANT);
    let mut s = slot(f, "v", 0, 0);
    let mut p = s.reborrow().init_type().init_any_pointer().init_parameter();
    p.set_scope_id(BOX);
    p.set_parameter_index(0);
    s.init_default_value().init_any_pointer();

    let mut usen = nodes.reborrow().get(USE_IX);
    node_header(usen.reborrow(), USE, FILE, "test.capnp:Use");
    let mut st = usen.init_struct();
    st.set_pointer_count(3);
    let mut fields = st.init_fields(3);
    for (i, name) in [(0u16, "b"), (1, "p"), (2, "u")] {
        let mut f = fields.reborrow().get(u32::from(i));
        f.set_discriminant_value(field::NO_DISCRIMINANT);
        let mut s = slot(f, name, i, u32::from(i));
        s.reborrow().init_default_value().init_struct();
        let mut t = s.init_type().init_struct();
        t.set_type_id(BOX);
        let brand = t.init_brand();
        if i == 2 {
            continue; // `Box`, unbound
        }
        let mut scope = brand.init_scopes(1).get(0);
        scope.set_scope_id(BOX);
        let mut binding = scope.init_bind(1).get(0).init_type();
        if i == 0 {
            binding.set_text(());
        } else {
            let mut b = binding.init_struct();
            b.set_type_id(POINT);
            b.init_brand();
        }
    }
}

/// The type bound in the brand of `Use` field `i`.
fn scope_binding<'a>(
    fields: &'a mut struct_list::Builder<'_, field::Owned>,
    i: u16,
) -> type_::Builder<'a> {
    let field::Slot(s) = fields.reborrow().get(u32::from(i)).which().unwrap() else {
        unreachable!()
    };
    let type_::Struct(t) = s.get_type().unwrap().which().unwrap() else {
        unreachable!()
    };
    let brand::scope::Bind(Ok(bindings)) = t
        .get_brand()
        .unwrap()
        .get_scopes()
        .unwrap()
        .get(0)
        .which()
        .unwrap()
    else {
        unreachable!()
    };
    bindings.get(0).init_type()
}

/// The valid batch, modified by `modify`.
fn batch(
    modify: impl FnOnce(&mut struct_list::Builder<'_, node::Owned>),
) -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let mut nodes: struct_list::Builder<'_, node::Owned> = message.initn_root(NODE_COUNT);
    build(&mut nodes);
    modify(&mut nodes);
    message
}

fn nodes(
    message: &message::Builder<message::HeapAllocator>,
) -> struct_list::Reader<'_, node::Owned> {
    message.get_root_as_reader().unwrap()
}

fn load_valid() -> SchemaLoader {
    let loader = SchemaLoader::new();
    let message = batch(|_| {});
    loader.load(nodes(&message)).unwrap();
    loader
}

fn struct_node<'a>(
    nodes: &'a mut struct_list::Builder<'_, node::Owned>,
    ix: u32,
) -> node::struct_::Builder<'a> {
    let node::Struct(st) = nodes.reborrow().get(ix).which().unwrap() else {
        panic!("not a struct")
    };
    st
}

fn field_of<'a>(
    nodes: &'a mut struct_list::Builder<'_, node::Owned>,
    ix: u32,
    field: u32,
) -> field::Builder<'a> {
    struct_node(nodes, ix).get_fields().unwrap().get(field)
}

fn slot_of<'a>(
    nodes: &'a mut struct_list::Builder<'_, node::Owned>,
    ix: u32,
    field: u32,
) -> field::slot::Builder<'a> {
    let field::Slot(s) = field_of(nodes, ix, field).which().unwrap() else {
        panic!("not a slot")
    };
    s
}

/// Loading the modified batch fails with an error mentioning `expected`, and loads nothing.
#[track_caller]
fn assert_rejected(
    expected: &str,
    modify: impl FnOnce(&mut struct_list::Builder<'_, node::Owned>),
) {
    let loader = SchemaLoader::new();
    let message = batch(modify);
    let err = loader.load(nodes(&message)).unwrap_err();
    let text = format!("{err}");
    assert!(text.contains(expected), "expected `{expected}` in `{text}`");
    for id in [FILE, POINT, COLOR, SHAPE, SHAPE_INFO, ANN, BOX, USE] {
        assert!(loader.get_node(id).is_err(), "node {id:#x} was loaded");
    }
}

fn text_type() -> Type {
    crate::text::Owned::introspect()
}

// ---------------------------------------------------------------------------------------------
// Semantics

#[test]
fn loads_and_reads_valid_batch() {
    let loader = load_valid();
    let point = loader.get_struct(POINT).unwrap();
    assert!(point.raw.is_loaded());
    let fields = point.get_fields().unwrap();
    assert_eq!(fields.len(), 5);
    assert_eq!(fields.get(0).get_type(), i32::introspect());
    assert_eq!(fields.get(2).get_type(), text_type());
    // Recursive: `next` is Point itself.
    assert_eq!(
        fields.get(3).get_type(),
        TypeVariant::Struct(point.raw).into()
    );
    let TypeVariant::Enum(color) = fields.get(4).get_type().which() else {
        panic!("expected an enum")
    };
    assert!(color.is_loaded());
    assert_eq!(EnumSchema::new(color), loader.get_enum(COLOR).unwrap());
    assert_eq!(point.get_field_by_name("label").unwrap().get_index(), 2);
    assert!(point.find_field_by_name("nope").unwrap().is_none());

    // Annotations, on the struct and on an enumerant.
    let a = point.get_annotations().unwrap().get(0);
    assert_eq!(a.get_type(), text_type());
    assert_eq!(
        a.get_value().unwrap().downcast::<crate::text::Reader<'_>>(),
        "point"
    );
    let green = loader
        .get_enum(COLOR)
        .unwrap()
        .get_enumerants()
        .unwrap()
        .get(1);
    let a = green.get_annotations().unwrap().get(0);
    assert_eq!(
        a.get_value().unwrap().downcast::<crate::text::Reader<'_>>(),
        "g"
    );

    // Other node kinds are reachable through `get_node`, but aren't structs or enums.
    assert!(loader.get_node(ANN).is_ok());
    assert!(loader.get_struct(ANN).is_err());
    assert!(loader.get_enum(POINT).is_err());
    assert!(loader.get_struct(COLOR).is_err());
    assert!(loader.get_struct(0xdead).is_err());
}

#[test]
fn reads_and_writes_through_loaded_schema() {
    let loader = load_valid();
    let point = loader.get_struct(POINT).unwrap();
    let mut message = message::Builder::new_default();
    {
        let mut root = message.init_root_dynamic(point).unwrap();
        root.set_named("x", 5i32.into()).unwrap();
        root.set_named("label", "hi".into()).unwrap();
        let next = root.reborrow().init_named("next").unwrap();
        let mut next: crate::dynamic_struct::Builder<'_> = next.downcast();
        next.set_named("y", 9i32.into()).unwrap();
        root.set_named(
            "color",
            crate::dynamic_value::Enum::new(1, loader.get_enum(COLOR).unwrap()).into(),
        )
        .unwrap();
    }
    let root = message.get_root_as_dynamic(point).unwrap();
    assert_eq!(
        format!("{:?}", root.into_reader()),
        "(x = 5, y = 7, label = \"hi\", next = (x = 0, y = 9, color = red), color = green)"
    );

    let shape = loader.get_struct(SHAPE).unwrap();
    let mut message = message::Builder::new_default();
    {
        let mut root = message.init_root_dynamic(shape).unwrap();
        root.set_named("square", 2.5f64.into()).unwrap();
        let info = root.reborrow().get_named("info").unwrap();
        let mut info: crate::dynamic_struct::Builder<'_> = info.downcast();
        info.set_named("a", 3u8.into()).unwrap();
        let mut info2 = root
            .init_named("info")
            .unwrap()
            .downcast::<crate::dynamic_struct::Builder<'_>>();
        info2.set_named("a", 4u8.into()).unwrap();
    }
    let root = message.get_root_as_dynamic(shape).unwrap().into_reader();
    assert_eq!(
        root.which()
            .unwrap()
            .unwrap()
            .get_proto()
            .get_name()
            .unwrap(),
        "square"
    );
    assert_eq!(format!("{root:?}"), "(square = 2.5, info = (a = 4))");
}

#[test]
fn loaded_schemas_are_interned() {
    let loader = load_valid();
    assert_eq!(
        loader.get_struct(POINT).unwrap(),
        loader.get_struct(POINT).unwrap()
    );
    assert_ne!(
        loader.get_struct(POINT).unwrap(),
        loader.get_struct(SHAPE).unwrap()
    );

    // Generic instances: `Use.b :Box(Text)` and `Use.p :Box(Point)` are distinct instances,
    // `Use.u :Box` is the default one.
    let usen = loader.get_struct(USE).unwrap();
    let fields = usen.get_fields().unwrap();
    let ty = |i| {
        let TypeVariant::Struct(raw) = fields.get(i).get_type().which() else {
            panic!("expected a struct")
        };
        StructSchema::new(raw)
    };
    let (b, p, u) = (ty(0), ty(1), ty(2));
    assert_eq!(u, loader.get_struct(BOX).unwrap());
    assert_ne!(b, u);
    assert_ne!(b, p);
    assert_eq!(b.get_brand_binding(BOX, 0), Some(text_type()));
    assert_eq!(b.get_fields().unwrap().get(0).get_type(), text_type());
    let point = TypeVariant::Struct(loader.get_struct(POINT).unwrap().raw).into();
    assert_eq!(p.get_brand_binding(BOX, 0), Some(point));
    assert_eq!(p.get_fields().unwrap().get(0).get_type(), point);
    assert_eq!(u.get_brand_binding(BOX, 0), Some(any_pointer()));
    assert_eq!(u.get_fields().unwrap().get(0).get_type(), any_pointer());
    assert_eq!(b.get_brand_scopes(), &[BrandScope::new(BOX, 1)]);
    assert_eq!(b.get_brand_binding(BOX, 1), None);
    assert_eq!(b.get_brand_binding(POINT, 0), None);

    // Reading the field again gives the same instance, and so does an explicit brand.
    assert_eq!(ty(0), b);
    let mut message = message::Builder::new_default();
    let mut brand: brand::Builder<'_> = message.init_root();
    let mut scope = brand.reborrow().init_scopes(1).get(0);
    scope.set_scope_id(BOX);
    scope.init_bind(1).get(0).init_type().set_text(());
    let branded = loader
        .get_struct_branded(BOX, brand.into_reader(), None)
        .unwrap();
    assert_eq!(branded, b);

    // `get_type` resolves through the same interner.
    let mut message = message::Builder::new_default();
    let mut ty: type_::Builder<'_> = message.init_root();
    let mut st = ty.reborrow().init_struct();
    st.set_type_id(BOX);
    let mut scope = st.init_brand().init_scopes(1).get(0);
    scope.set_scope_id(BOX);
    scope.init_bind(1).get(0).init_type().set_text(());
    let resolved = loader.get_type(ty.reborrow_as_reader(), None).unwrap();
    assert_eq!(resolved, TypeVariant::Struct(b.raw).into());

    // A parameter resolves in the scope it is looked up in.
    let mut p = ty
        .init_list()
        .init_element_type()
        .init_any_pointer()
        .init_parameter();
    p.set_scope_id(BOX);
    p.set_parameter_index(0);
    let ty = message.get_root_as_reader::<type_::Reader<'_>>().unwrap();
    assert_eq!(
        loader.get_type(ty, Some(b)).unwrap(),
        Type::list_of(text_type())
    );
    assert_eq!(
        loader.get_type(ty, None).unwrap(),
        Type::list_of(any_pointer())
    );
}

#[test]
fn two_loaders_are_unequal() {
    let a = load_valid();
    let b = load_valid();
    assert_ne!(a.get_struct(POINT).unwrap(), b.get_struct(POINT).unwrap());
    assert_ne!(a.get_enum(COLOR).unwrap(), b.get_enum(COLOR).unwrap());
    // And neither is equal to a compiled schema with the same shape.
    let compiled: Type = crate::schema_capnp::node::Owned::introspect();
    assert_ne!(
        Type::from(TypeVariant::Struct(a.get_struct(POINT).unwrap().raw)),
        compiled
    );
}

#[test]
fn identical_reload_is_deduplicated() {
    let loader = load_valid();
    let before = loader.get_struct(POINT).unwrap();
    let words = lock(loader.inner).nodes[&POINT].words.as_ptr();
    let message = batch(|_| {});
    loader.load(nodes(&message)).unwrap();
    // A batch may also overlap: here, only Point again.
    loader.load([nodes(&message).get(POINT_IX)]).unwrap();
    assert_eq!(lock(loader.inner).nodes[&POINT].words.as_ptr(), words);
    assert_eq!(loader.get_struct(POINT).unwrap(), before);
    assert_eq!(lock(loader.inner).nodes.len(), NODE_COUNT as usize);
}

#[test]
fn conflicting_reload_is_rejected() {
    let loader = load_valid();
    let message = batch(|nodes| {
        slot_of(nodes, POINT_IX, 1)
            .get_default_value()
            .unwrap()
            .set_int32(8);
    });
    let err = loader.load(nodes(&message)).unwrap_err();
    assert!(format!("{err}").contains("different content"));
    // The original is still there.
    let point = loader.get_struct(POINT).unwrap();
    let message = message::Builder::new_default();
    let root = message
        .get_root_as_reader::<crate::any_pointer::Reader<'_>>()
        .unwrap();
    assert_eq!(
        format!("{:?}", root.get_as_dynamic(point).unwrap()),
        "(x = 0, y = 7, color = red)"
    );
}

#[test]
fn batches_can_build_on_each_other() {
    let loader = SchemaLoader::new();
    let message = batch(|_| {});
    let list = nodes(&message);
    // Point needs Color and the annotation; loading it first fails, and loads nothing.
    assert!(loader.load([list.get(POINT_IX)]).is_err());
    assert!(loader.get_node(POINT).is_err());
    loader
        .load([list.get(FILE_IX), list.get(COLOR_IX), list.get(ANN_IX)])
        .unwrap();
    loader.load([list.get(POINT_IX)]).unwrap();
    assert_eq!(
        loader
            .get_struct(POINT)
            .unwrap()
            .get_fields()
            .unwrap()
            .get(4)
            .get_type(),
        TypeVariant::Enum(loader.get_enum(COLOR).unwrap().raw).into()
    );
}

#[test]
fn schema_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SchemaLoader>();
    assert_send_sync::<Type>();
    assert_send_sync::<TypeVariant>();
    assert_send_sync::<RawBrandedStructSchema>();
    assert_send_sync::<RawEnumSchema>();
}

#[test]
fn loaded_types_work_across_threads() {
    let loader = load_valid();
    let usen: Type = TypeVariant::Struct(loader.get_struct(USE).unwrap().raw).into();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            std::thread::spawn(move || {
                let TypeVariant::Struct(raw) = usen.which() else {
                    unreachable!()
                };
                let fields = StructSchema::new(raw).get_fields().unwrap();
                (0..3).map(|i| fields.get(i).get_type()).collect::<Vec<_>>()
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results[0], results[1]);
}

// ---------------------------------------------------------------------------------------------
// Validation

#[test]
fn rejects_discriminant_out_of_range() {
    assert_rejected("invalid or duplicate discriminant", |nodes| {
        field_of(nodes, SHAPE_IX, 1).set_discriminant_value(2);
    });
}

#[test]
fn rejects_duplicate_discriminant() {
    assert_rejected("invalid or duplicate discriminant", |nodes| {
        field_of(nodes, SHAPE_IX, 1).set_discriminant_value(0);
    });
}

#[test]
fn rejects_discriminant_count_mismatch() {
    assert_rejected("discriminantCount", |nodes| {
        let mut st = struct_node(nodes, SHAPE_IX);
        st.set_discriminant_count(3);
    });
    assert_rejected("at least two members", |nodes| {
        struct_node(nodes, SHAPE_IX).set_discriminant_count(1);
    });
    assert_rejected("discriminant is out of bounds", |nodes| {
        struct_node(nodes, SHAPE_IX).set_discriminant_offset(8);
    });
}

#[test]
fn rejects_offsets_out_of_bounds() {
    assert_rejected("offset is out of bounds", |nodes| {
        slot_of(nodes, POINT_IX, 1).set_offset(4); // Int32 #4 needs 160 bits; there are 128
    });
    assert_rejected("offset is out of bounds", |nodes| {
        slot_of(nodes, POINT_IX, 3).set_offset(2); // pointer #2 of 2
    });
    assert_rejected("offset is out of bounds", |nodes| {
        struct_node(nodes, POINT_IX).set_data_word_count(0);
    });
    // A group's members are checked against its own sizes.
    assert_rejected("offset is out of bounds", |nodes| {
        slot_of(nodes, SHAPE_INFO_IX, 0).set_offset(16);
    });
}

#[test]
fn rejects_group_larger_than_parent() {
    assert_rejected("larger than its parent", |nodes| {
        struct_node(nodes, SHAPE_INFO_IX).set_data_word_count(3);
    });
}

#[test]
fn rejects_group_not_scoped_to_parent() {
    assert_rejected("not a group of this struct", |nodes| {
        nodes.reborrow().get(SHAPE_INFO_IX).set_scope_id(POINT);
    });
    assert_rejected("not a group of this struct", |nodes| {
        struct_node(nodes, SHAPE_INFO_IX).set_is_group(false);
    });
    assert_rejected("is not a group of this struct", |nodes| {
        let field::Group(mut g) = field_of(nodes, SHAPE_IX, 2).which().unwrap() else {
            unreachable!()
        };
        g.set_type_id(POINT);
    });
}

#[test]
fn rejects_wrong_kind_reference() {
    assert_rejected("as a struct, but it is an enum", |nodes| {
        let type_::Struct(mut t) = slot_of(nodes, POINT_IX, 3)
            .get_type()
            .unwrap()
            .which()
            .unwrap()
        else {
            unreachable!()
        };
        t.set_type_id(COLOR);
    });
    assert_rejected("as an enum, but it is a struct", |nodes| {
        let type_::Enum(mut t) = slot_of(nodes, POINT_IX, 4)
            .get_type()
            .unwrap()
            .which()
            .unwrap()
        else {
            unreachable!()
        };
        t.set_type_id(SHAPE);
    });
    assert_rejected("as an annotation, but it is a struct", |nodes| {
        let node::Struct(_) = nodes.reborrow().get(POINT_IX).which().unwrap() else {
            unreachable!()
        };
        nodes
            .reborrow()
            .get(POINT_IX)
            .get_annotations()
            .unwrap()
            .get(0)
            .set_id(SHAPE);
    });
    // A newtype id naming a struct.
    assert_rejected("as a `type` newtype, but it is a struct", |nodes| {
        field_of(nodes, POINT_IX, 0).set_type_id(POINT);
    });
}

#[test]
fn rejects_missing_dependency() {
    assert_rejected("which is not loaded", |nodes| {
        let type_::Struct(mut t) = slot_of(nodes, POINT_IX, 3)
            .get_type()
            .unwrap()
            .which()
            .unwrap()
        else {
            unreachable!()
        };
        t.set_type_id(0xdead);
    });
    assert_rejected("which is not loaded", |nodes| {
        let field::Group(mut g) = field_of(nodes, SHAPE_IX, 2).which().unwrap() else {
            unreachable!()
        };
        g.set_type_id(0xdead);
    });
    assert_rejected("which is not loaded", |nodes| {
        nodes
            .reborrow()
            .get(POINT_IX)
            .get_annotations()
            .unwrap()
            .get(0)
            .set_id(0xdead);
    });
}

#[test]
fn rejects_default_value_mismatch() {
    assert_rejected("default value does not match", |nodes| {
        slot_of(nodes, POINT_IX, 0)
            .get_default_value()
            .unwrap()
            .set_int64(0);
    });
    assert_rejected("default value does not match", |nodes| {
        slot_of(nodes, POINT_IX, 2)
            .get_default_value()
            .unwrap()
            .set_data(&[]);
    });
    assert_rejected("default value does not match", |nodes| {
        slot_of(nodes, BOX_IX, 0)
            .get_default_value()
            .unwrap()
            .set_text("");
    });
}

#[test]
fn rejects_duplicate_names_and_code_orders() {
    assert_rejected("duplicate member name", |nodes| {
        field_of(nodes, POINT_IX, 1).set_name("x");
    });
    assert_rejected("invalid codeOrder", |nodes| {
        field_of(nodes, POINT_IX, 1).set_code_order(0);
    });
    assert_rejected("invalid codeOrder", |nodes| {
        let node::Enum(e) = nodes.reborrow().get(COLOR_IX).which().unwrap() else {
            unreachable!()
        };
        e.get_enumerants().unwrap().get(1).set_code_order(5);
    });
}

#[test]
fn rejects_bad_generics() {
    // A parameter of a scope the node isn't in.
    assert_rejected("is not in scope", |nodes| {
        let type_::AnyPointer(a) = slot_of(nodes, BOX_IX, 0)
            .get_type()
            .unwrap()
            .which()
            .unwrap()
        else {
            unreachable!()
        };
        let type_::any_pointer::Parameter(mut p) = a.which().unwrap() else {
            unreachable!()
        };
        p.set_scope_id(POINT);
    });
    // A parameter index out of range.
    assert_rejected("is not in scope", |nodes| {
        let type_::AnyPointer(a) = slot_of(nodes, BOX_IX, 0)
            .get_type()
            .unwrap()
            .which()
            .unwrap()
        else {
            unreachable!()
        };
        let type_::any_pointer::Parameter(mut p) = a.which().unwrap() else {
            unreachable!()
        };
        p.set_parameter_index(1);
    });
    // A binding to a non-pointer type.
    assert_rejected("non-pointer type", |nodes| {
        let mut fields = struct_node(nodes, USE_IX).get_fields().unwrap();
        scope_binding(&mut fields, 0).set_uint32(());
    });
    // A brand for a scope the target isn't in.
    assert_rejected("not one of its scopes", |nodes| {
        let field::Slot(s) = field_of(nodes, USE_IX, 0).which().unwrap() else {
            unreachable!()
        };
        let type_::Struct(t) = s.get_type().unwrap().which().unwrap() else {
            unreachable!()
        };
        t.get_brand()
            .unwrap()
            .get_scopes()
            .unwrap()
            .get(0)
            .set_scope_id(USE);
    });
    // Parameters on a node not marked generic.
    assert_rejected("isGeneric", |nodes| {
        nodes.reborrow().get(BOX_IX).set_is_generic(false);
    });
}

#[test]
fn rejects_alias_cycles() {
    let loader = SchemaLoader::new();
    let mut message = message::Builder::new_default();
    let mut list: struct_list::Builder<'_, node::Owned> = message.initn_root(3);
    let mut file = list.reborrow().get(0);
    node_header(file.reborrow(), FILE, 0, "f");
    file.set_file(());
    for (i, (id, aliased)) in [(0x2001, 0x2002), (0x2002, 0x2001)].into_iter().enumerate() {
        let mut n = list.reborrow().get(i as u32 + 1);
        node_header(n.reborrow(), id, FILE, "alias");
        let mut t = n.init_type();
        t.set_type_id(aliased);
        t.set_text(());
    }
    let err = loader.load(list.into_reader()).unwrap_err();
    assert!(format!("{err}").contains("alias cycle"), "{err}");
}

#[test]
fn rejects_scope_cycles() {
    assert_rejected("has a cycle", |nodes| {
        nodes.reborrow().get(FILE_IX).set_scope_id(SHAPE_INFO);
        nodes.reborrow().get(SHAPE_IX).set_scope_id(SHAPE_INFO);
    });
}

#[test]
fn rejects_duplicate_ids_in_a_batch() {
    let loader = SchemaLoader::new();
    let a = batch(|_| {});
    let b = batch(|nodes| {
        slot_of(nodes, POINT_IX, 1)
            .get_default_value()
            .unwrap()
            .set_int32(8)
    });
    let err = loader
        .load(nodes(&a).iter().chain([nodes(&b).get(POINT_IX)]))
        .unwrap_err();
    assert!(format!("{err}").contains("twice"), "{err}");
    // The same node twice with the same content is fine.
    loader
        .load(nodes(&a).iter().chain([nodes(&a).get(POINT_IX)]))
        .unwrap();
}

#[test]
fn rejects_unknown_node_kind() {
    // An encoding from a newer schema.capnp: a union discriminant this crate doesn't know.
    let loader = SchemaLoader::new();
    let message = batch(|_| {});
    let mut copy = canonical_copy(nodes(&message).get(POINT_IX)).unwrap();
    // Node's union discriminant is the UInt16 at byte 12 of its data section (word 1 of the
    // message: word 0 is the root pointer).
    let bytes = Word::words_to_bytes_mut(&mut copy);
    bytes[8 + 12] = 0x55;
    let reader = message::Reader::new(vec![Word::words_to_bytes(&copy)], Default::default());
    let node: node::Reader<'_> = reader.get_root().unwrap();
    assert!(loader.load([node]).is_err());
}

// ---------------------------------------------------------------------------------------------
// Fuzz-ish: random corruption of the valid batch must give an error or a schema whose dynamic
// API does not panic.

/// Exercises the dynamic API over every struct the loader has, on an arbitrary message.
fn exercise(loader: &SchemaLoader, ids: &[u64], data: &[Word]) {
    for &id in ids {
        let Ok(schema) = loader.get_struct(id) else {
            continue;
        };
        let reader = message::Reader::new(vec![Word::words_to_bytes(data)], Default::default());
        if let Ok(root) = reader.get_root_as_dynamic(schema) {
            let _ = format!("{root:?}");
        }
        let mut message = message::Builder::new_default();
        let mut root = message.init_root_dynamic(schema).unwrap();
        if let Ok(fields) = schema.get_fields() {
            for field in fields {
                let _ = root
                    .reborrow()
                    .get(field)
                    .map(|v| format!("{:?}", v.into_reader()));
                let _ = root.clear(field);
                let _ = root.reborrow().init(field);
                let _ = root.reborrow().initn(field, 2);
                let _ = root.has(field);
                if let Ok(annotations) = field.get_annotations() {
                    for a in annotations {
                        let _ = a.get_value().map(|v| format!("{v:?}"));
                    }
                }
            }
        }
        let _ = format!("{:?}", root.into_reader());
        if let Ok(annotations) = schema.get_annotations() {
            for a in annotations {
                let _ = a.get_value().map(|v| format!("{v:?}"));
            }
        }
    }
}

#[test]
fn corrupted_nodes_do_not_panic() {
    use quickcheck::{QuickCheck, TestResult};

    fn prop(corruptions: Vec<(u8, u16, u64)>, data: Vec<u64>) -> TestResult {
        let message = batch(|_| {});
        let list = nodes(&message);
        let mut copies: Vec<Vec<Word>> = list.iter().map(|n| canonical_copy(n).unwrap()).collect();
        // One or two small corruptions per case, so that a fair share still loads.
        for (node, word, value) in corruptions.into_iter().take(2) {
            let n = copies.len();
            let copy = &mut copies[usize::from(node) % n];
            // Leave the root pointer alone, so the node is still readable as a struct.
            if copy.len() > 1 {
                let i = 1 + usize::from(word) % (copy.len() - 1);
                let bytes = Word::words_to_bytes_mut(&mut copy[i..=i]);
                // Mostly small corruptions: flip one byte.
                let b = (value % 8) as usize;
                bytes[b] ^= ((value >> 8) as u8) | 1;
            }
        }
        let readers: Vec<_> = copies
            .iter()
            .map(|c| message::Reader::new(vec![Word::words_to_bytes(c)], Default::default()))
            .collect();
        let mut node_readers = Vec::new();
        for r in &readers {
            match r.get_root::<node::Reader<'_>>() {
                Ok(n) => node_readers.push(n),
                Err(_) => return TestResult::discard(),
            }
        }
        let loader = SchemaLoader::new();
        if loader.load(node_readers).is_ok() {
            let mut data: Vec<Word> = data
                .into_iter()
                .map(|w| Word {
                    raw_content: w.to_le_bytes(),
                })
                .collect();
            if data.is_empty() {
                data.push(Word {
                    raw_content: [0; 8],
                });
            }
            exercise(&loader, &[POINT, SHAPE, SHAPE_INFO, BOX, USE], &data);
        }
        TestResult::passed()
    }

    let tests = if cfg!(miri) { 4 } else { 1000 };
    QuickCheck::new()
        .tests(tests)
        .max_tests(tests * 10)
        .quickcheck(prop as fn(Vec<(u8, u16, u64)>, Vec<u64>) -> TestResult);
}

#[test]
fn valid_batch_survives_exercise() {
    let loader = load_valid();
    let data = [
        Word {
            raw_content: [0, 0, 0, 0, 2, 0, 3, 0],
        },
        Word {
            raw_content: [0xff; 8],
        },
        Word {
            raw_content: [1; 8],
        },
    ];
    exercise(&loader, &[POINT, SHAPE, SHAPE_INFO, BOX, USE], &data);
}

#[test]
fn usable_as_is_equality_for_compiled_and_unrelated_types() {
    let text = text_type();
    let data = crate::data::Owned::introspect();
    assert!(text.usable_as(text));
    assert!(!text.usable_as(data));
    assert!(Type::list_of(text).usable_as(Type::list_of(text)));
    assert!(!Type::list_of(text).usable_as(text));
    assert!(!Type::list_of(Type::list_of(text)).usable_as(Type::list_of(data)));
    let node = node::Owned::introspect();
    assert!(node.usable_as(node));
    assert!(!node.usable_as(field::Owned::introspect()));
}

#[test]
// Loads all of schema.capnp, which takes a long time under Miri.
#[cfg_attr(miri, ignore)]
fn load_compiled_inserts_generated_nodes() {
    // The bootstrap schema is generated code too: load `Node` and everything it reaches from
    // it alone, then read a node through the loaded schema and downcast it.
    let loader = SchemaLoader::new();
    loader.load_compiled::<node::Owned>().unwrap();
    // Idempotent.
    loader.load_compiled::<node::Owned>().unwrap();
    let TypeVariant::Struct(compiled) = node::Owned::introspect().which() else {
        unreachable!()
    };
    let id = StructSchema::new(compiled).get_proto().get_id();
    let schema = loader.get_struct(id).unwrap();
    assert!(RawBrandedStructSchema::from(schema).is_loaded());

    let mut message = message::Builder::new_default();
    {
        let mut n = message.init_root::<node::Builder<'_>>();
        node_header(n.reborrow(), 0x42, 0, "x");
        n.init_struct().set_data_word_count(3);
    }
    let root = message
        .get_root_as_reader::<crate::any_pointer::Reader<'_>>()
        .unwrap()
        .get_as_dynamic(schema)
        .unwrap();
    assert_eq!(root.try_downcast::<node::Owned>().unwrap().get_id(), 0x42);
    assert!(root.try_downcast::<field::Owned>().is_err());
    // A group of it.
    let st: crate::dynamic_struct::Reader<'_> = root.get_named("struct").unwrap().downcast();
    let st = st.try_downcast::<node::struct_::Owned>().unwrap();
    assert_eq!(st.get_data_word_count(), 3);
    // And a builder.
    let root = message.get_root_as_dynamic(schema).unwrap();
    let mut typed = root.try_downcast::<node::Owned>().unwrap();
    typed.set_id(0x43);
    assert_eq!(
        message
            .get_root_as_reader::<node::Reader<'_>>()
            .unwrap()
            .get_id(),
        0x43
    );
}
