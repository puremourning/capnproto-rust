// Copyright (c) 2026 the Cap'n Proto authors and contributors
// Licensed under the MIT License:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

//! `capnp::schema_loader` over the CodeGeneratorRequests build.rs dumps: schemas loaded at run
//! time describe the same structure as the compiled-in ones, and the dynamic API reads and
//! writes the same data through them.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use capnp::introspect::{Introspect, RawBrandedStructSchema, Type, TypeVariant};
use capnp::private::newtype_layout;
use capnp::schema::{EnumSchema, StructSchema};
use capnp::schema_capnp::{brand, code_generator_request, node, type_};
use capnp::schema_loader::SchemaLoader;
use capnp::{any_pointer, data, dynamic_struct, dynamic_value, message, serialize, text};

use crate::test_capnp::{
    test_all_types, test_defaults, test_generics, test_generics_groups, test_generics_wrapper2,
    test_groups, test_implicit_method_params, test_interface, test_union, test_use_generics,
    TestEnum,
};
use crate::test_newtype_capnp::{
    aliases, cross_file, envelope, order_type, place_params, place_results, point, price, priced,
    shapes, status, vec3,
};
use crate::test_newtype_evolution_v1_capnp as v1;
use crate::test_newtype_evolution_v2_capnp as v2;
use crate::test_util::{self, CheckTestMessage};

static CGR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/raw_code_gen_request.bin"));
static EVOLUTION_V1_CGR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/evolution-v1.cgr"));
static EVOLUTION_V2_CGR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/evolution-v2.cgr"));
static SCHEMA_LOADER_CGR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/schema-loader.cgr"));

fn read_cgr(bytes: &[u8]) -> message::Reader<serialize::OwnedSegments> {
    let mut options = message::ReaderOptions::new();
    options.traversal_limit_in_words(None);
    serialize::read_message(bytes, options).unwrap()
}

fn load_into(loader: &SchemaLoader, bytes: &[u8]) -> capnp::Result<()> {
    let message = read_cgr(bytes);
    let request: code_generator_request::Reader<'_> = message.get_root().unwrap();
    loader.load_code_generator_request(request)
}

fn load(bytes: &[u8]) -> SchemaLoader {
    let loader = SchemaLoader::new();
    load_into(&loader, bytes).unwrap();
    loader
}

/// One loader per request, shared by the tests (loading test.capnp is the slow part under
/// Miri).
fn loader() -> &'static SchemaLoader {
    static LOADER: OnceLock<SchemaLoader> = OnceLock::new();
    LOADER.get_or_init(|| load(CGR))
}

fn v1_loader() -> &'static SchemaLoader {
    static LOADER: OnceLock<SchemaLoader> = OnceLock::new();
    LOADER.get_or_init(|| load(EVOLUTION_V1_CGR))
}

fn v2_loader() -> &'static SchemaLoader {
    static LOADER: OnceLock<SchemaLoader> = OnceLock::new();
    LOADER.get_or_init(|| load(EVOLUTION_V2_CGR))
}

fn compiled<T: Introspect>() -> StructSchema {
    struct_of(T::introspect())
}

fn id_of<T: Introspect>() -> u64 {
    compiled::<T>().get_proto().get_id()
}

/// The loaded counterpart of `T`'s node, unbranded.
fn loaded<T: Introspect>() -> StructSchema {
    loader().get_struct(id_of::<T>()).unwrap()
}

fn struct_of(ty: Type) -> StructSchema {
    let TypeVariant::Struct(raw) = ty.which() else {
        panic!("not a struct type: {ty:?}");
    };
    StructSchema::new(raw)
}

fn is_loaded(schema: StructSchema) -> bool {
    RawBrandedStructSchema::from(schema).is_loaded()
}

fn field_type(schema: StructSchema, name: &str) -> Type {
    schema.get_field_by_name(name).unwrap().get_type()
}

/// A structural description of a type: node ids and brand bindings, recursively. Equal for a
/// compiled type and its loaded counterpart.
fn describe(ty: Type) -> String {
    describe_at(ty, 0)
}

fn describe_at(ty: Type, depth: u32) -> String {
    match ty.which() {
        TypeVariant::Struct(raw) => describe_struct_at(StructSchema::new(raw), depth),
        TypeVariant::Enum(raw) => format!("Enum({:#x})", EnumSchema::new(raw).get_proto().get_id()),
        TypeVariant::List(element) => format!("List({})", describe_at(element, depth)),
        other => format!("{other:?}"),
    }
}

fn describe_struct(schema: StructSchema) -> String {
    describe_struct_at(schema, 0)
}

fn describe_struct_at(schema: StructSchema, depth: u32) -> String {
    let mut out = format!("Struct({:#x}", schema.get_proto().get_id());
    if depth > 16 {
        return out + " ...)";
    }
    for scope in schema.get_brand_scopes() {
        out += &format!(" {:#x}:[", scope.scope_id());
        for i in 0..scope.param_count() {
            let binding = schema
                .get_brand_binding(scope.scope_id(), i)
                .expect("a binding for every scope parameter");
            if i > 0 {
                out += ", ";
            }
            out += &describe_at(binding, depth + 1);
        }
        out += "]";
    }
    out + ")"
}

/// `compiled` and `loaded` describe the same struct: fields, types, bindings, newtypes and
/// annotations, recursively through struct-typed fields.
fn assert_equivalent(compiled: StructSchema, loaded: StructSchema, seen: &mut HashSet<String>) {
    let key = describe_struct(compiled);
    assert_eq!(key, describe_struct(loaded));
    if !seen.insert(key.clone()) {
        return;
    }
    assert!(!is_loaded(compiled));
    assert!(is_loaded(loaded));
    assert_ne!(compiled, loaded);
    let name = compiled
        .get_proto()
        .get_display_name()
        .unwrap()
        .to_string()
        .unwrap();
    assert_eq!(
        compiled.get_brand_scopes(),
        loaded.get_brand_scopes(),
        "{name}"
    );
    assert_eq!(
        compiled.get_newtype_ids(),
        loaded.get_newtype_ids(),
        "{name}"
    );
    let (compiled_ids, compiled_table) = newtype_layout::recorded_layout(compiled);
    let (loaded_ids, loaded_table) = newtype_layout::recorded_layout(loaded);
    assert_eq!(compiled_ids, loaded_ids, "{name}");
    assert_eq!(compiled_table.copied(), loaded_table.copied(), "{name}");
    assert_annotations_equivalent(
        compiled.get_annotations().unwrap(),
        loaded.get_annotations().unwrap(),
    );

    let compiled_fields = compiled.get_fields().unwrap();
    let loaded_fields = loaded.get_fields().unwrap();
    assert_eq!(compiled_fields.len(), loaded_fields.len(), "{name}");
    for (c, l) in compiled_fields.iter().zip(loaded_fields) {
        let field_name = c.get_proto().get_name().unwrap().to_str().unwrap();
        assert_eq!(
            field_name,
            l.get_proto().get_name().unwrap().to_str().unwrap()
        );
        assert_eq!(c.get_index(), l.get_index());
        assert_eq!(
            describe(c.get_type()),
            describe(l.get_type()),
            "{name}.{field_name}"
        );
        assert_eq!(
            c.get_newtype_ids(),
            l.get_newtype_ids(),
            "{name}.{field_name}"
        );
        assert_eq!(loaded.get_field_by_name(field_name).unwrap(), l);
        assert_annotations_equivalent(c.get_annotations().unwrap(), l.get_annotations().unwrap());

        let (mut ct, mut lt) = (c.get_type(), l.get_type());
        while let (TypeVariant::List(ce), TypeVariant::List(le)) = (ct.which(), lt.which()) {
            (ct, lt) = (ce, le);
        }
        match (ct.which(), lt.which()) {
            (TypeVariant::Struct(cs), TypeVariant::Struct(ls)) => {
                assert_equivalent(StructSchema::new(cs), StructSchema::new(ls), seen)
            }
            (TypeVariant::Enum(ce), TypeVariant::Enum(le)) => {
                let (ce, le) = (EnumSchema::new(ce), EnumSchema::new(le));
                assert_ne!(ce, le);
                let names = |e: EnumSchema| -> Vec<String> {
                    e.get_enumerants()
                        .unwrap()
                        .iter()
                        .map(|e| e.get_proto().get_name().unwrap().to_string().unwrap())
                        .collect()
                };
                assert_eq!(names(ce), names(le));
            }
            _ => {}
        }
    }
    let indices =
        |s: capnp::schema::FieldSubset| s.iter().map(|f| f.get_index()).collect::<Vec<_>>();
    assert_eq!(
        indices(compiled.get_union_fields().unwrap()),
        indices(loaded.get_union_fields().unwrap())
    );
    assert_eq!(
        indices(compiled.get_non_union_fields().unwrap()),
        indices(loaded.get_non_union_fields().unwrap())
    );
}

fn assert_annotations_equivalent(
    compiled: capnp::schema::AnnotationList,
    loaded: capnp::schema::AnnotationList,
) {
    assert_eq!(compiled.len(), loaded.len());
    for (c, l) in compiled.iter().zip(loaded) {
        assert_eq!(c.get_id(), l.get_id());
        assert_eq!(describe(c.get_type()), describe(l.get_type()));
        assert_eq!(
            format!("{:?}", c.get_value().unwrap()),
            format!("{:?}", l.get_value().unwrap())
        );
    }
}

/// Every field of a dynamic value, read through `get` (so defaults show), to `depth` levels.
fn dump(value: dynamic_value::Reader<'_>, depth: u32) -> String {
    match value {
        dynamic_value::Reader::Struct(s) if depth > 0 => {
            let mut out = String::from("(");
            // Errors are part of the output: compiled and loaded schemas must agree on them too.
            // (A generic field bound to an interface, for one, reads as a type mismatch.)
            let get = |field| match s.get(field) {
                Ok(v) => dump(v, depth - 1),
                Err(e) => format!("<{:?}>", e.kind),
            };
            for field in s.get_schema().get_non_union_fields().unwrap() {
                out += &format!(
                    "{} = {}, ",
                    field.get_proto().get_name().unwrap().to_str().unwrap(),
                    get(field)
                );
            }
            if let Some(field) = s.which().unwrap() {
                out += &format!(
                    "| {} = {}",
                    field.get_proto().get_name().unwrap().to_str().unwrap(),
                    get(field)
                );
            }
            out + ")"
        }
        dynamic_value::Reader::List(l) if depth > 0 => {
            let items: Vec<_> = l.iter().map(|v| dump(v.unwrap(), depth - 1)).collect();
            format!("[{}]", items.join(", "))
        }
        dynamic_value::Reader::Struct(_) => "(..)".into(),
        dynamic_value::Reader::List(_) => "[..]".into(),
        other => format!("{other:?}"),
    }
}

/// The message's root, as an `AnyPointer`.
fn root_pointer<A: message::Allocator>(message: &message::Builder<A>) -> any_pointer::Reader<'_> {
    message.get_root_as_reader().unwrap()
}

/// `T`'s root read through the compiled and the loaded schema print the same.
fn assert_reads_same<T: Introspect, A: message::Allocator>(message: &message::Builder<A>) {
    let root = root_pointer(message);
    let compiled_root = root.get_as_dynamic(compiled::<T>()).unwrap();
    let loaded_root = root.get_as_dynamic(loaded::<T>()).unwrap();
    assert_eq!(format!("{compiled_root:?}"), format!("{loaded_root:?}"));
    assert_eq!(dump(compiled_root.into(), 4), dump(loaded_root.into(), 4));
}

fn cgr_node_id(bytes: &[u8], name: &str) -> u64 {
    let message = read_cgr(bytes);
    let request: code_generator_request::Reader<'_> = message.get_root().unwrap();
    request
        .get_nodes()
        .unwrap()
        .iter()
        .find(|n| n.get_display_name().unwrap() == name)
        .unwrap_or_else(|| panic!("no node {name}"))
        .get_id()
}

// ---------------------------------------------------------------------------------------------
// Identity

#[test]
fn loaded_schemas_equal_themselves_not_compiled() {
    let a = loaded::<test_all_types::Owned>();
    assert!(is_loaded(a));
    assert_eq!(a, loaded::<test_all_types::Owned>());
    assert_ne!(a, compiled::<test_all_types::Owned>());
    assert_ne!(a, loaded::<test_defaults::Owned>());
    // A field naming the struct gives the same instance as a lookup.
    assert_eq!(struct_of(field_type(a, "structField")), a);
    assert_eq!(
        field_type(a, "structList"),
        Type::list_of(TypeVariant::Struct(a.into()).into())
    );
    assert_ne!(
        Type::from(TypeVariant::Struct(a.into())),
        test_all_types::Owned::introspect()
    );
    // Enums too.
    let TypeVariant::Enum(e) = field_type(a, "enumField").which() else {
        panic!("not an enum")
    };
    assert!(e.is_loaded());
    let TypeVariant::Enum(c) = TestEnum::introspect().which() else {
        panic!("not an enum")
    };
    assert_ne!(e, c);
    let TypeVariant::Enum(e2) = field_type(loaded::<test_defaults::Owned>(), "enumField").which()
    else {
        panic!("not an enum")
    };
    assert_eq!(e, e2);
    assert_eq!(
        EnumSchema::new(e),
        loader()
            .get_enum(EnumSchema::new(c).get_proto().get_id())
            .unwrap()
    );
}

#[test]
fn two_loaders_are_unequal() {
    let other = load(EVOLUTION_V1_CGR);
    let id = id_of::<v1::holder::Owned>();
    let a = v1_loader().get_struct(id).unwrap();
    let b = other.get_struct(id).unwrap();
    assert_ne!(a, b);
    assert_ne!(field_type(a, "a"), field_type(b, "a"));
    // Both read the same data the same way.
    let message = crate::newtype_evolution::v1_message();
    let root = root_pointer(&message);
    assert_eq!(
        format!("{:?}", root.get_as_dynamic(a).unwrap()),
        format!("{:?}", root.get_as_dynamic(b).unwrap())
    );
}

#[test]
fn reload_is_deduplicated_and_conflicts_are_rejected() {
    let loader = load(EVOLUTION_V1_CGR);
    let id = id_of::<v1::holder::Owned>();
    let before = loader.get_struct(id).unwrap();
    load_into(&loader, EVOLUTION_V1_CGR).unwrap();
    assert_eq!(loader.get_struct(id).unwrap(), before);

    // Version 2 reuses version 1's newtype ids with different content: the whole batch is
    // rejected, and version 1 is untouched.
    let err = load_into(&loader, EVOLUTION_V2_CGR).unwrap_err();
    assert!(format!("{err}").contains("different content"), "{err}");
    assert_eq!(loader.get_struct(id).unwrap(), before);
    let v2_file = cgr_node_id(EVOLUTION_V2_CGR, "test-newtype-evolution-v2.capnp");
    assert!(loader.get_node(v2_file).is_err());
}

#[test]
fn hash_map_of_loaded_fields() {
    let schema = loaded::<test_all_types::Owned>();
    let mut map = HashMap::new();
    for field in schema.get_fields().unwrap() {
        map.insert(field, field.get_index());
    }
    for field in compiled::<test_all_types::Owned>().get_fields().unwrap() {
        map.insert(field, 1000 + field.get_index());
    }
    let fields = schema.get_fields().unwrap();
    assert_eq!(map.len(), 2 * fields.len() as usize);
    for name in ["int8Field", "textField", "structList"] {
        let field = loaded::<test_all_types::Owned>()
            .get_field_by_name(name)
            .unwrap();
        assert_eq!(map[&field], field.get_index());
    }
    let mut schemas = HashSet::new();
    schemas.insert(loaded::<test_all_types::Owned>());
    schemas.insert(loaded::<test_all_types::Owned>());
    schemas.insert(compiled::<test_all_types::Owned>());
    assert_eq!(schemas.len(), 2);
}

#[test]
#[should_panic]
fn downcast_of_loaded_struct_to_compiled_type_panics() {
    // `downcast::<T>()` compares schema identity, and a loaded schema is never a compiled one.
    // (Making loaded values usable as compiled types is a separate, later feature.)
    let mut message = message::Builder::new_default();
    message.init_root::<test_all_types::Builder<'_>>();
    let root = root_pointer(&message)
        .get_as_dynamic(loaded::<test_all_types::Owned>())
        .unwrap();
    let _ = root.downcast::<test_all_types::Owned>();
}

#[test]
fn other_node_kinds() {
    let interface = loader()
        .get_node(<test_interface::Client as capnp::traits::HasTypeId>::TYPE_ID)
        .unwrap();
    assert!(matches!(interface.which().unwrap(), node::Interface(_)));
    assert!(loader()
        .get_struct(<test_interface::Client as capnp::traits::HasTypeId>::TYPE_ID)
        .is_err());
    assert!(loader().get_enum(id_of::<test_all_types::Owned>()).is_err());
    let ann = loader().get_node(test_generics::ann::ID).unwrap();
    assert!(matches!(ann.which().unwrap(), node::Annotation(_)));
    // Constants: TestConstants' nested `structConst`.
    let constants = cgr_node_id(CGR, "test.capnp:TestConstants.structConst");
    assert!(matches!(
        loader().get_node(constants).unwrap().which().unwrap(),
        node::Const(_)
    ));
}

// ---------------------------------------------------------------------------------------------
// Conformance: the same structure, and the same reads and writes

#[test]
fn structure_matches_compiled() {
    let mut seen = HashSet::new();
    let roots = [
        compiled::<test_all_types::Owned>(),
        compiled::<test_defaults::Owned>(),
        compiled::<test_union::Owned>(),
        compiled::<test_groups::Owned>(),
        compiled::<crate::test_capnp::test_unnamed_union::Owned>(),
        compiled::<crate::test_capnp::test_union_defaults::Owned>(),
        compiled::<test_use_generics::Owned>(),
        compiled::<test_generics_wrapper2::Owned>(),
        // `$Rust.name` annotations don't change the loaded name lookup table.
        compiled::<crate::test_capnp::renamed_struct::Owned>(),
        compiled::<test_generics_groups::Owned<any_pointer::Owned, any_pointer::Owned>>(),
        compiled::<
            crate::test_capnp::test_generics_union::Owned<any_pointer::Owned, any_pointer::Owned>,
        >(),
        compiled::<shapes::Owned>(),
        compiled::<aliases::Owned>(),
        compiled::<cross_file::Owned>(),
        compiled::<place_params::Owned>(),
        compiled::<place_results::Owned>(),
        compiled::<envelope::Owned<any_pointer::Owned>>(),
    ];
    for root in roots {
        let loaded = loader().get_struct(root.get_proto().get_id()).unwrap();
        assert_equivalent(root, loaded, &mut seen);
    }
    // Instances reached through fields, generic ones included.
    assert!(seen.len() > 40, "{}", seen.len());
}

#[test]
fn test_all_types_reads_same() {
    let mut message = message::Builder::new_default();
    test_util::init_test_message(message.init_root());
    assert_reads_same::<test_all_types::Owned, _>(&message);

    // `dynamic_check_test_message` checks what `dynamic_init_test_message` writes.
    let mut message = message::Builder::new_default();
    let root: dynamic_value::Builder<'_> =
        message.init_root::<test_all_types::Builder<'_>>().into();
    test_util::dynamic_init_test_message(root.downcast());
    assert_reads_same::<test_all_types::Owned, _>(&message);
    let root = message
        .get_root_as_reader::<any_pointer::Reader<'_>>()
        .unwrap()
        .get_as_dynamic(loaded::<test_all_types::Owned>())
        .unwrap();
    test_util::dynamic_check_test_message(root);
}

#[test]
fn defaults_read_same() {
    let message = message::Builder::new_default();
    assert_reads_same::<test_defaults::Owned, _>(&message);
    let root = root_pointer(&message)
        .get_as_dynamic(loaded::<test_defaults::Owned>())
        .unwrap();
    test_util::dynamic_check_test_message(root);

    // Defaults through a loaded builder.
    let mut message = message::Builder::new_default();
    let root = message
        .get_root_as_dynamic(loaded::<test_defaults::Owned>())
        .unwrap();
    test_util::dynamic_check_test_message_builder(root);
}

#[test]
fn unions_read_same() {
    let mut message = message::Builder::new_default();
    let mut root: test_union::Builder<'_> = message.init_root();
    root.reborrow().get_union0().set_u0f1s32(1234567);
    root.reborrow().get_union1().set_u1f1sp("foo");
    root.reborrow().get_union2().set_u2f0s1(true);
    root.reborrow()
        .get_union3()
        .set_u3f0s64(1234567890123456789);
    assert_reads_same::<test_union::Owned, _>(&message);

    let root = root_pointer(&message)
        .get_as_dynamic(loaded::<test_union::Owned>())
        .unwrap();
    let u: dynamic_struct::Reader<'_> = root.get_named("union1").unwrap().downcast();
    assert_eq!(
        u.which().unwrap().unwrap().get_proto().get_name().unwrap(),
        "u1f1sp"
    );
    assert!(u.has_named("u1f1sp").unwrap());
    assert!(!u.has_named("u1f0sp").unwrap());

    let message = message::Builder::new_default();
    assert_reads_same::<crate::test_capnp::test_union_defaults::Owned, _>(&message);
}

#[test]
fn groups_read_same() {
    let mut message = message::Builder::new_default();
    let root: test_groups::Builder<'_> = message.init_root();
    let mut bar = root.get_groups().init_bar();
    bar.set_corge(23);
    bar.set_grault("hello");
    bar.set_garply(-7);
    assert_reads_same::<test_groups::Owned, _>(&message);

    let mut message = message::Builder::new_default();
    let mut root: crate::test_capnp::test_unnamed_union::Builder<'_> = message.init_root();
    root.set_before("before");
    root.set_bar(321);
    root.set_middle(5);
    assert_reads_same::<crate::test_capnp::test_unnamed_union::Owned, _>(&message);
}

#[test]
fn generics_read_same() {
    // Empty: every field reads its (generic, nested) default.
    let message = message::Builder::new_default();
    assert_reads_same::<test_use_generics::Owned, _>(&message);

    let mut message = message::Builder::new_default();
    {
        let mut root: test_use_generics::Builder<'_> = message.init_root();
        let mut basic = root.reborrow().init_basic();
        test_util::init_test_message(basic.reborrow().init_foo());
        basic.reborrow().init_rev().init_bar().set_text_field("rev");
        let mut inner2 = root.reborrow().init_inner2();
        inner2.set_baz("baz").unwrap();
        inner2.init_inner_bound().init_foo().set_int32_field(3);
        root.reborrow()
            .init_wrapper()
            .init_value()
            .init_foo()
            .set_u_int8_field(9);
    }
    assert_reads_same::<test_use_generics::Owned, _>(&message);

    // Generic groups.
    let mut message = message::Builder::new_default();
    {
        let mut root: test_generics_groups::Builder<'_, text::Owned, data::Owned> =
            message.init_root();
        root.set_foo("foo").unwrap();
        root.get_bar().set_baz(&[1, 2, 3][..]).unwrap();
    }
    let root = root_pointer(&message);
    let compiled_root = root
        .get_as_dynamic(compiled::<
            test_generics_groups::Owned<text::Owned, data::Owned>,
        >())
        .unwrap();
    let loaded_root = root
        .get_as_dynamic(text_data_instance(id_of::<
            test_generics_groups::Owned<any_pointer::Owned, any_pointer::Owned>,
        >()))
        .unwrap();
    assert_eq!(format!("{compiled_root:?}"), format!("{loaded_root:?}"));
    assert_eq!(
        format!("{loaded_root:?}"),
        "(foo = \"foo\", bar = (baz = 0x\"010203\"))"
    );
}

#[test]
fn newtypes_read_same() {
    let mut message = message::Builder::new_default();
    {
        let mut root: shapes::Builder<'_> = message.init_root();
        root.reborrow().get_top_left().set_x(1.0);
        root.reborrow().get_bottom_right().set_z(2.0);
        root.reborrow().get_named().set_label("label");
        root.reborrow().get_status().set_code(5);
        root.reborrow().get_prices().get_stop().set_value(9);
        root.reborrow().get_order().init_limit().set_scale(3);
        root.reborrow().get_priced_partial().set_amount(4);
        root.set_id(&[1, 2]);
        root.set_age(30);
    }
    assert_reads_same::<shapes::Owned, _>(&message);
}

#[test]
fn loaded_builder_writes_what_compiled_code_reads() {
    let mut message = message::Builder::new_default();
    let root = message
        .init_root_dynamic(loaded::<test_all_types::Owned>())
        .unwrap();
    test_util::dynamic_init_test_message(root);
    // Read back through compiled code, dynamically and typed.
    let reader = message
        .get_root_as_reader::<test_all_types::Reader<'_>>()
        .unwrap();
    test_util::dynamic_check_test_message(dynamic_value::Reader::from(reader).downcast());
    assert_eq!(reader.get_text_field().unwrap(), "foo");
    assert_eq!(reader.get_struct_list().unwrap().len(), 3);
    assert_eq!(reader.get_enum_field().unwrap(), TestEnum::Corge);

    // And the typed initializer's message, copied through a loaded reader into a loaded
    // builder, reads back as the typed check expects.
    let mut source = message::Builder::new_default();
    test_util::init_test_message(source.init_root());
    let mut message = message::Builder::new_default();
    {
        let src = root_pointer(&source)
            .get_as_dynamic(loaded::<test_all_types::Owned>())
            .unwrap();
        let mut dst = message
            .init_root_dynamic(loaded::<test_all_types::Owned>())
            .unwrap();
        for field in loaded::<test_all_types::Owned>().get_fields().unwrap() {
            if src.has(field).unwrap() {
                dst.set(field, src.get(field).unwrap()).unwrap();
            }
        }
    }
    CheckTestMessage::check_test_message(
        message
            .get_root_as_reader::<test_all_types::Reader<'_>>()
            .unwrap(),
    );

    // Unions, groups and generics through a loaded builder.
    let mut message = message::Builder::new_default();
    {
        let mut root = message
            .init_root_dynamic(loaded::<test_union::Owned>())
            .unwrap();
        let mut u: dynamic_struct::Builder<'_> =
            root.reborrow().get_named("union1").unwrap().downcast();
        u.set_named("u1f1sp", "foo".into()).unwrap();
        let mut u: dynamic_struct::Builder<'_> = root.get_named("union3").unwrap().downcast();
        u.set_named("u3f0s64", 77i64.into()).unwrap();
    }
    let reader = message
        .get_root_as_reader::<test_union::Reader<'_>>()
        .unwrap();
    match reader.get_union1().which().unwrap() {
        test_union::union1::U1f1sp(t) => assert_eq!(t.unwrap(), "foo"),
        _ => panic!("expected u1f1sp"),
    }
    match reader.get_union3().which().unwrap() {
        test_union::union3::U3f0s64(x) => assert_eq!(x, 77),
        _ => panic!("expected u3f0s64"),
    }

    let mut message = message::Builder::new_default();
    {
        let mut root = message
            .init_root_dynamic(loaded::<test_use_generics::Owned>())
            .unwrap();
        let mut basic: dynamic_struct::Builder<'_> =
            root.reborrow().init_named("basic").unwrap().downcast();
        test_util::dynamic_init_test_message(
            basic.reborrow().init_named("foo").unwrap().downcast(),
        );
        let mut inner2: dynamic_struct::Builder<'_> = root.init_named("inner2").unwrap().downcast();
        inner2.set_named("baz", "baz".into()).unwrap();
    }
    let reader = message
        .get_root_as_reader::<test_use_generics::Reader<'_>>()
        .unwrap();
    test_util::dynamic_check_test_message(
        dynamic_value::Reader::from(reader.get_basic().unwrap().get_foo().unwrap()).downcast(),
    );
    assert_eq!(reader.get_inner2().unwrap().get_baz().unwrap(), "baz");
}

#[test]
fn generic_annotation() {
    let annotations = loaded::<test_use_generics::Owned>()
        .get_annotations()
        .unwrap();
    assert_eq!(annotations.len(), 1);
    let ann = annotations.get(0);
    assert_eq!(ann.get_id(), test_generics::ann::ID);
    // `$TestGenerics(Text, Data).ann("foo")`: the annotation's type is its `Foo`, bound to Text.
    assert_eq!(ann.get_type(), text::Owned::introspect());
    assert_eq!(
        ann.get_value().unwrap().downcast::<text::Reader<'_>>(),
        "foo"
    );
}

// ---------------------------------------------------------------------------------------------
// Generics

/// A `Brand` binding each `(scope, types)` pair, built by `bind(i, type)`.
fn brand_message(
    scopes: &[(u64, usize)],
    bind: impl Fn(u64, u32, type_::Builder<'_>),
) -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let brand: brand::Builder<'_> = message.init_root();
    let mut list = brand.init_scopes(scopes.len() as u32);
    for (i, &(scope_id, count)) in scopes.iter().enumerate() {
        let mut scope = list.reborrow().get(i as u32);
        scope.set_scope_id(scope_id);
        let mut bindings = scope.init_bind(count as u32);
        for j in 0..count as u32 {
            bind(scope_id, j, bindings.reborrow().get(j).init_type());
        }
    }
    message
}

/// The loaded instance of generic struct `id` (two parameters, its own) bound to `(Text, Data)`.
fn text_data_instance(id: u64) -> StructSchema {
    let message = brand_message(&[(id, 2)], |_, j, mut t| {
        if j == 0 {
            t.set_text(())
        } else {
            t.set_data(())
        }
    });
    loader()
        .get_struct_branded(id, message.get_root_as_reader().unwrap(), None)
        .unwrap()
}

fn set_struct_type(t: type_::Builder<'_>, id: u64) {
    let mut s = t.init_struct();
    s.set_type_id(id);
    s.init_brand();
}

#[test]
fn branded_lookup_matches_compiled_bindings() {
    type Generics = test_generics::Owned<text::Owned, data::Owned>;
    let generics_id = id_of::<Generics>();
    let instance = text_data_instance(generics_id);
    assert_eq!(
        describe_struct(instance),
        describe_struct(compiled::<Generics>())
    );
    assert_eq!(field_type(instance, "foo"), text::Owned::introspect());
    assert_eq!(field_type(instance, "bar"), data::Owned::introspect());
    assert_eq!(
        instance.get_brand_binding(generics_id, 0),
        Some(text::Owned::introspect())
    );
    assert_eq!(instance.get_brand_binding(generics_id, 2), None);
    assert_eq!(instance.get_brand_binding(generics_id + 1, 0), None);
    // `rev :TestGenerics(Bar, Foo)` swaps the bindings.
    let rev = struct_of(field_type(instance, "rev"));
    assert_eq!(field_type(rev, "foo"), data::Owned::introspect());
    assert_eq!(struct_of(field_type(rev, "rev")), instance);
    assert_equivalent(compiled::<Generics>(), instance, &mut HashSet::new());

    // Interned: the same lookup twice, and the same instance reached through a field.
    assert_eq!(text_data_instance(generics_id), instance);
    let unbound = loader().get_struct(generics_id).unwrap();
    assert_ne!(unbound, instance);
    assert_eq!(field_type(unbound, "foo"), any_pointer::Owned::introspect());
    assert_eq!(
        unbound.get_brand_binding(generics_id, 1),
        Some(any_pointer::Owned::introspect())
    );
}

#[test]
fn nested_scopes_and_inherited_bindings() {
    type Generics = test_generics::Owned<text::Owned, data::Owned>;
    type Inner2 = test_generics::inner2::Owned<text::Owned, data::Owned, test_all_types::Owned>;
    type DeepNest = test_generics::inner2::deep_nest::Owned<
        text::Owned,
        data::Owned,
        test_all_types::Owned,
        capnp::primitive_list::Owned<u32>,
    >;
    let generics_id = id_of::<Generics>();
    let inner2_id = id_of::<Inner2>();
    let deep_nest_id = id_of::<DeepNest>();
    let all_types_id = id_of::<test_all_types::Owned>();
    let message = brand_message(
        &[(generics_id, 2), (inner2_id, 1)],
        |scope, j, mut t| match (scope == generics_id, j) {
            (true, 0) => t.set_text(()),
            (true, _) => t.set_data(()),
            (false, _) => set_struct_type(t, all_types_id),
        },
    );
    let inner2 = loader()
        .get_struct_branded(inner2_id, message.get_root_as_reader().unwrap(), None)
        .unwrap();
    assert_eq!(
        describe_struct(inner2),
        describe_struct(compiled::<Inner2>())
    );
    assert_equivalent(compiled::<Inner2>(), inner2, &mut HashSet::new());

    // DeepNest(Qux) inherits the scopes it doesn't bind from its lookup scope.
    {
        let mut deep = message::Builder::new_default();
        let brand: brand::Builder<'_> = deep.init_root();
        let mut scopes = brand.init_scopes(3);
        for (i, sid) in [generics_id, inner2_id].into_iter().enumerate() {
            let mut s = scopes.reborrow().get(i as u32);
            s.set_scope_id(sid);
            s.set_inherit(());
        }
        let mut s = scopes.reborrow().get(2);
        s.set_scope_id(deep_nest_id);
        s.init_bind(1)
            .get(0)
            .init_type()
            .init_list()
            .init_element_type()
            .set_uint32(());
        let deep_nest = loader()
            .get_struct_branded(
                deep_nest_id,
                deep.get_root_as_reader().unwrap(),
                Some(inner2),
            )
            .unwrap();
        assert_eq!(
            describe_struct(deep_nest),
            describe_struct(compiled::<DeepNest>())
        );
        assert_eq!(field_type(deep_nest, "foo"), text::Owned::introspect());
        assert_eq!(
            field_type(deep_nest, "qux"),
            capnp::primitive_list::Owned::<u32>::introspect()
        );
        // Without the scope, inherited parameters are unbound.
        let unscoped = loader()
            .get_struct_branded(deep_nest_id, deep.get_root_as_reader().unwrap(), None)
            .unwrap();
        assert_eq!(
            field_type(unscoped, "foo"),
            any_pointer::Owned::introspect()
        );
        assert_ne!(unscoped, deep_nest);
    }

    // `TestGenerics(Text, Data).Inner` binds its parent's scope.
    let inner = struct_of(field_type(inner2, "innerBound"));
    assert_eq!(
        describe_struct(inner),
        describe_struct(compiled::<
            test_generics::inner::Owned<text::Owned, data::Owned>,
        >())
    );
    assert_eq!(field_type(inner, "bar"), data::Owned::introspect());
    // `innerUnbound :TestGenerics.Inner` doesn't.
    let unbound = struct_of(field_type(inner2, "innerUnbound"));
    assert_eq!(field_type(unbound, "bar"), any_pointer::Owned::introspect());
}

#[test]
fn groups_take_their_parents_bindings() {
    let id = id_of::<test_generics_groups::Owned<any_pointer::Owned, any_pointer::Owned>>();
    let instance = text_data_instance(id);
    let bar = struct_of(field_type(instance, "bar"));
    assert_eq!(field_type(bar, "baz"), data::Owned::introspect());
    assert_eq!(
        describe_struct(bar),
        describe_struct(struct_of(field_type(
            compiled::<test_generics_groups::Owned<text::Owned, data::Owned>>(),
            "bar"
        )))
    );
    assert_eq!(
        bar.get_brand_binding(id, 1),
        Some(data::Owned::introspect())
    );
    // The unbound instance's group is unbound too, and a different instance.
    let unbound_bar = struct_of(field_type(loader().get_struct(id).unwrap(), "bar"));
    assert_eq!(
        field_type(unbound_bar, "baz"),
        any_pointer::Owned::introspect()
    );
    assert_ne!(unbound_bar, bar);
}

#[test]
fn interfaces_and_implicit_parameters() {
    // `cap :TestGenerics(TestInterface, Text)`: an interface binding reads as a capability.
    let cap = struct_of(field_type(loaded::<test_use_generics::Owned>(), "cap"));
    assert_eq!(field_type(cap, "foo"), TypeVariant::Capability.into());

    // `call @0 [T, U] (foo :T, bar :U) -> TestGenerics(T, U)`: implicit method parameters are
    // `AnyPointer`.
    let interface = loader()
        .get_node(<test_implicit_method_params::Client as capnp::traits::HasTypeId>::TYPE_ID)
        .unwrap();
    let node::Interface(interface) = interface.which().unwrap() else {
        panic!("not an interface")
    };
    let call = interface.get_methods().unwrap().get(0);
    let params = loader().get_struct(call.get_param_struct_type()).unwrap();
    assert_eq!(field_type(params, "foo"), any_pointer::Owned::introspect());
    let results = loader()
        .get_struct_branded(
            call.get_result_struct_type(),
            call.get_result_brand().unwrap(),
            None,
        )
        .unwrap();
    assert_eq!(field_type(results, "foo"), any_pointer::Owned::introspect());
    assert_eq!(
        results,
        loader().get_struct(call.get_result_struct_type()).unwrap()
    );
}

#[test]
fn get_type_resolves_in_scope() {
    let generics_id = id_of::<test_generics::Owned<any_pointer::Owned, any_pointer::Owned>>();
    let instance = text_data_instance(generics_id);
    let mut message = message::Builder::new_default();
    {
        let ty: type_::Builder<'_> = message.init_root();
        let mut p = ty
            .init_list()
            .init_element_type()
            .init_any_pointer()
            .init_parameter();
        p.set_scope_id(generics_id);
        p.set_parameter_index(1);
    }
    let ty: type_::Reader<'_> = message.get_root_as_reader().unwrap();
    assert_eq!(
        loader().get_type(ty, Some(instance)).unwrap(),
        Type::list_of(data::Owned::introspect())
    );
    assert_eq!(
        loader().get_type(ty, None).unwrap(),
        Type::list_of(any_pointer::Owned::introspect())
    );
    // And with a compiled scope.
    assert_eq!(
        loader()
            .get_type(
                ty,
                Some(compiled::<test_generics::Owned<text::Owned, data::Owned>>())
            )
            .unwrap(),
        Type::list_of(data::Owned::introspect())
    );

    // A struct type naming a missing node is an error.
    let mut message = message::Builder::new_default();
    set_struct_type(message.init_root(), 0x1234);
    assert!(loader()
        .get_type(message.get_root_as_reader().unwrap(), None)
        .is_err());
}

#[test]
fn polymorphic_recursion() {
    let loader = load(SCHEMA_LOADER_CGR);
    let id = |name: &str| {
        cgr_node_id(
            SCHEMA_LOADER_CGR,
            &format!("test-schema-loader.capnp:{name}"),
        )
    };
    let (nest_id, box_id) = (id("Nest"), id("Box"));
    let use_nest = loader.get_struct(id("UseNest")).unwrap();

    // Nest(Text) -> Nest(Box(Text)) -> Nest(Box(Box(Text))) -> ...: each level is a new
    // instance, made when first reached.
    let mut nest = struct_of(field_type(use_nest, "nest"));
    let mut value = text::Owned::introspect();
    let mut expected = String::from("Text");
    for _ in 0..6 {
        assert_eq!(field_type(nest, "value"), value);
        assert_eq!(nest.get_brand_binding(nest_id, 0), Some(value));
        assert_eq!(describe(value), expected);
        let child = struct_of(field_type(nest, "child"));
        assert_ne!(child, nest);
        assert_eq!(struct_of(field_type(nest, "child")), child);
        value = child.get_brand_binding(nest_id, 0).unwrap();
        assert_eq!(field_type(struct_of(value), "v"), field_type(nest, "value"));
        expected = format!("Struct({box_id:#x} {box_id:#x}:[{expected}])");
        nest = child;
    }

    // Read and written dynamically.
    let mut message = message::Builder::new_default();
    {
        let root = message.init_root_dynamic(use_nest).unwrap();
        let mut nest: dynamic_struct::Builder<'_> = root.init_named("nest").unwrap().downcast();
        nest.set_named("value", "a".into()).unwrap();
        let mut child: dynamic_struct::Builder<'_> = nest.init_named("child").unwrap().downcast();
        let mut value: dynamic_struct::Builder<'_> =
            child.reborrow().init_named("value").unwrap().downcast();
        value.set_named("v", "b".into()).unwrap();
        let grandchild: dynamic_struct::Builder<'_> = child.init_named("child").unwrap().downcast();
        let value: dynamic_struct::Builder<'_> = grandchild.init_named("value").unwrap().downcast();
        let mut inner: dynamic_struct::Builder<'_> = value.init_named("v").unwrap().downcast();
        inner.set_named("v", "c".into()).unwrap();
    }
    let root = root_pointer(&message).get_as_dynamic(use_nest).unwrap();
    assert_eq!(
        format!("{root:?}"),
        "(nest = (value = \"a\", child = (value = (v = \"b\"), child = (value = (v = (v = \"c\"))))))"
    );
}

// ---------------------------------------------------------------------------------------------
// Newtypes

#[test]
fn use_site_tables_match_compiled() {
    let mut use_sites = 0;
    let mut seen = HashSet::new();
    for root in [
        compiled::<shapes::Owned>(),
        compiled::<cross_file::Owned>(),
        compiled::<aliases::Owned>(),
        compiled::<place_params::Owned>(),
        compiled::<place_results::Owned>(),
        compiled::<envelope::Owned<any_pointer::Owned>>(),
    ] {
        let mut stack = vec![root];
        while let Some(schema) = stack.pop() {
            let id = schema.get_proto().get_id();
            if !seen.insert(id) {
                continue;
            }
            let loaded = loader().get_struct(id).unwrap();
            let (compiled_ids, compiled_table) = newtype_layout::recorded_layout(schema);
            let (loaded_ids, loaded_table) = newtype_layout::recorded_layout(loaded);
            assert_eq!(compiled_ids, loaded_ids);
            assert_eq!(compiled_table.copied(), loaded_table.copied());
            if compiled_table.is_some() {
                use_sites += 1;
            }
            for field in schema.get_fields().unwrap() {
                if let TypeVariant::Struct(raw) = field.get_type().which() {
                    stack.push(StructSchema::new(raw));
                }
            }
        }
    }
    // test-newtype.capnp generates 31 use-site tables.
    assert_eq!(use_sites, 31);
}

fn loaded_shapes_root<A: message::Allocator>(
    message: &message::Builder<A>,
) -> dynamic_struct::Reader<'_> {
    root_pointer(message)
        .get_as_dynamic(loaded::<shapes::Owned>())
        .unwrap()
}

#[test]
fn newtype_downcasts_on_loaded_groups() {
    let mut message = message::Builder::new_default();
    {
        let mut root: shapes::Builder<'_> = message.init_root();
        root.reborrow().get_top_left().set_y(1.5);
        root.reborrow().get_status().set_label("lbl");
        root.reborrow().get_prices().get_limit().set_value(42);
        root.reborrow().get_order().init_limit().set_value(43);
        root.reborrow().get_priced_partial().set_amount(5);
    }
    let root = loaded_shapes_root(&message);
    let group = |name| root.get_named(name).unwrap();

    let top_left: vec3::AnyReader<'_> = group("topLeft").downcast();
    assert_eq!(top_left.get_y(), 1.5);
    // A union newtype.
    let status: status::AnyReader<'_> = group("status").downcast();
    match status.which().unwrap() {
        status::Which::Label(t) => assert_eq!(t.unwrap(), "lbl"),
        _ => panic!("expected label"),
    }
    // A nested newtype member, and a union arm that is one.
    let dynamic_value::Reader::Struct(prices) = group("prices") else {
        panic!("not a group")
    };
    let limit: price::AnyReader<'_> = prices.get_named("limit").unwrap().downcast();
    assert_eq!(limit.get_value(), 42);
    let order: order_type::AnyReader<'_> = group("order").downcast();
    match order.which().unwrap() {
        order_type::Which::Limit(p) => assert_eq!(p.get_value(), 43),
        _ => panic!("expected limit"),
    }
    // An incomplete mapping: `scale` is unmapped and reads its default.
    let partial: priced::AnyReader<'_> = group("pricedPartial").downcast();
    assert_eq!((partial.get_amount(), partial.get_scale()), (5, 100));
    let dynamic_value::Reader::Struct(top_left) = group("topLeft") else {
        panic!("not a group")
    };
    assert_eq!(top_left.get_schema().get_newtype_ids(), &[vec3::TYPE_ID]);

    // Through an alias chain: ChainedPoint -> Point -> Vec3.
    let mut message = message::Builder::new_default();
    message
        .init_root::<aliases::Builder<'_>>()
        .get_chained()
        .set_y(2.5);
    let root = root_pointer(&message)
        .get_as_dynamic(loaded::<aliases::Owned>())
        .unwrap();
    let chained: point::AnyReader<'_> = root.get_named("chained").unwrap().downcast();
    assert_eq!(chained.get_y(), 2.5);
    let chained: vec3::AnyReader<'_> = root.get_named("chained").unwrap().downcast();
    assert_eq!(chained.get_y(), 2.5);
}

#[test]
fn newtype_builder_downcasts_on_loaded_groups() {
    let mut message = message::Builder::new_default();
    {
        let mut root = message
            .init_root_dynamic(loaded::<shapes::Owned>())
            .unwrap();
        let mut bottom_right: vec3::AnyBuilder<'_> =
            root.reborrow().get_named("bottomRight").unwrap().downcast();
        bottom_right.set_x(10.0);
        bottom_right.set_z(30.0);
        let mut order: order_type::AnyBuilder<'_> =
            root.reborrow().get_named("order").unwrap().downcast();
        order.set_cancel(7);
        let prices: dynamic_struct::Builder<'_> =
            root.reborrow().get_named("prices").unwrap().downcast();
        let mut stop: price::AnyBuilder<'_> = prices.get_named("stop").unwrap().downcast();
        stop.set_scale(2);
    }
    let reader = message.get_root_as_reader::<shapes::Reader<'_>>().unwrap();
    assert_eq!(reader.reborrow().get_bottom_right().get_x(), 10.0);
    assert_eq!(reader.reborrow().get_bottom_right().get_z(), 30.0);
    assert_eq!(reader.reborrow().get_top_left().get_x(), 0.0);
    assert_eq!(reader.reborrow().get_prices().get_stop().get_scale(), 2);
    match reader.get_order().which().unwrap() {
        shapes::order::Cancel(n) => assert_eq!(n, 7),
        _ => panic!("expected cancel"),
    }
}

#[test]
#[should_panic(expected = "was not declared as type")]
fn newtype_downcast_on_loaded_group_rejects_other_newtypes() {
    let mut message = message::Builder::new_default();
    message.init_root::<shapes::Builder<'_>>();
    let root = loaded_shapes_root(&message);
    let _: status::AnyReader<'_> = root.get_named("topLeft").unwrap().downcast();
}

#[test]
#[should_panic(expected = "not mapped")]
fn newtype_incomplete_mapping_set_panics_on_loaded_group() {
    let mut message = message::Builder::new_default();
    let mut root = message
        .init_root_dynamic(loaded::<shapes::Owned>())
        .unwrap();
    let mut partial: priced::AnyBuilder<'_> = root
        .reborrow()
        .get_named("pricedPartial")
        .unwrap()
        .downcast();
    partial.set_scale(1);
}

fn evolution_root<'a, A: message::Allocator>(
    loader: &SchemaLoader,
    message: &'a message::Builder<A>,
    holder_id: u64,
) -> dynamic_struct::Reader<'a> {
    root_pointer(message)
        .get_as_dynamic(loader.get_struct(holder_id).unwrap())
        .unwrap()
}

#[test]
fn loaded_v2_use_site_through_v1_carriers() {
    let message = crate::newtype_evolution::v2_message();
    let root = evolution_root(v2_loader(), &message, id_of::<v2::holder::Owned>());
    let group = |name| root.get_named(name).unwrap();
    crate::newtype_evolution::check_v1_view(
        group("a").downcast(),
        group("l").downcast(),
        group("s").downcast(),
        group("r").downcast(),
    );
}

#[test]
fn loaded_v1_use_site_through_v2_carriers() {
    let message = crate::newtype_evolution::v1_message();
    let root = evolution_root(v1_loader(), &message, id_of::<v1::holder::Owned>());
    let group = |name| root.get_named(name).unwrap();
    crate::newtype_evolution::check_v2_view_of_v1(
        group("a").downcast(),
        group("l").downcast(),
        group("s").downcast(),
        group("r").downcast(),
    );
}

#[test]
fn loaded_v2_use_site_written_through_v1_carriers() {
    let mut message = crate::newtype_evolution::v2_message();
    {
        let schema = v2_loader()
            .get_struct(id_of::<v2::holder::Owned>())
            .unwrap();
        let mut root = message.get_root_as_dynamic(schema).unwrap();
        let mut a: v1::vec::AnyBuilder<'_> = root.reborrow().get_named("a").unwrap().downcast();
        a.set_y(-2.0);
        let r: v1::span::AnyBuilder<'_> = root.reborrow().get_named("r").unwrap().downcast();
        r.get_br().set_y(-11.0);
    }
    let h = message
        .get_root_as_reader::<v2::holder::Reader<'_>>()
        .unwrap();
    assert_eq!(
        (h.get_a().get_x(), h.get_a().get_y(), h.get_a().get_z()),
        (1.0, -2.0, 21.0)
    );
    assert_eq!(h.get_r().get_br().get_y(), -11.0);
}

#[test]
fn loaded_evolution_tables_match_compiled() {
    for (loader, compiled_holder) in [
        (v1_loader(), compiled::<v1::holder::Owned>()),
        (v2_loader(), compiled::<v2::holder::Owned>()),
    ] {
        let loaded = loader
            .get_struct(compiled_holder.get_proto().get_id())
            .unwrap();
        assert_equivalent(compiled_holder, loaded, &mut HashSet::new());
    }
    // The repro from plans/newtype-evolution-fixes.md, a struct of its own.
    for (loader, compiled_s) in [
        (v1_loader(), compiled::<v1::s::Owned>()),
        (v2_loader(), compiled::<v2::s::Owned>()),
    ] {
        let loaded = loader.get_struct(compiled_s.get_proto().get_id()).unwrap();
        assert_equivalent(compiled_s, loaded, &mut HashSet::new());
    }
}

#[test]
fn name_lookup_ignores_rust_name_annotations() {
    // The loader sorts its name table by schema name, which is what `find_field_by_name`
    // searches. (Generated code sorts by the `$Rust.name`, so the compiled lookup of `bar` here
    // misses; that is a codegen issue, not checked here.)
    let id = id_of::<crate::test_capnp::renamed_struct::Owned>();
    let schema = loader().get_struct(id).unwrap();
    for field in schema.get_fields().unwrap() {
        let name = field.get_proto().get_name().unwrap().to_str().unwrap();
        assert_eq!(
            schema.find_field_by_name(name).unwrap(),
            Some(field),
            "{name}"
        );
    }
}
