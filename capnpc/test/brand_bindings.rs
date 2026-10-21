//! `StructSchema::get_brand_binding` / `get_brand_scopes` on compiled code.

use crate::test_capnp::{
    test_all_types, test_generics, test_generics_groups, test_generics_wrapper, test_use_generics,
};
use capnp::introspect::{BrandScope, Introspect, Type, TypeVariant};
use capnp::schema::StructSchema;
use capnp::{any_pointer, data, text};

fn schema_of<T: Introspect>() -> StructSchema {
    struct_schema(T::introspect())
}

fn struct_schema(ty: Type) -> StructSchema {
    match ty.which() {
        TypeVariant::Struct(raw) => raw.into(),
        _ => panic!("not a struct type"),
    }
}

fn ty<T: Introspect>() -> Type {
    T::introspect()
}

fn id_of<T: Introspect>() -> u64 {
    schema_of::<T>().get_proto().get_id()
}

#[test]
fn non_generic_struct_has_no_bindings() {
    let schema = schema_of::<test_all_types::Owned>();
    assert!(schema.get_brand_scopes().is_empty());
    let id = schema.get_proto().get_id();
    assert_eq!(schema.get_brand_binding(id, 0), None);
    assert_eq!(schema.get_brand_binding(0, 0), None);

    let schema = schema_of::<test_use_generics::Owned>();
    assert!(schema.get_brand_scopes().is_empty());
}

#[test]
fn own_scope() {
    let generics_id = id_of::<test_generics::Owned<any_pointer::Owned, any_pointer::Owned>>();
    let schema = schema_of::<test_generics::Owned<text::Owned, data::Owned>>();

    assert_eq!(
        schema.get_brand_scopes(),
        &[BrandScope::new(generics_id, 2)]
    );
    assert_eq!(
        schema.get_brand_binding(generics_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        schema.get_brand_binding(generics_id, 1),
        Some(ty::<data::Owned>())
    );
    assert_eq!(schema.get_brand_binding(generics_id, 2), None);
    assert_eq!(schema.get_brand_binding(generics_id + 1, 0), None);

    // Unbound parameters (`TestGenerics` with no brand) are `AnyPointer`.
    let unbound = schema_of::<test_generics::Owned<any_pointer::Owned, any_pointer::Owned>>();
    assert_eq!(
        unbound.get_brand_binding(generics_id, 0),
        Some(ty::<any_pointer::Owned>())
    );
}

#[test]
fn nested_scopes() {
    type Outer = test_generics::Owned<text::Owned, data::Owned>;
    type Inner2 = test_generics::inner2::Owned<text::Owned, data::Owned, test_all_types::Owned>;
    type DeepNest = test_generics::inner2::deep_nest::Owned<
        text::Owned,
        data::Owned,
        test_all_types::Owned,
        capnp::primitive_list::Owned<u32>,
    >;
    let outer_id = id_of::<Outer>();
    let inner2_id = id_of::<Inner2>();
    let deep_nest_id = id_of::<DeepNest>();

    // `TestGenerics(Foo, Bar).Inner2(Baz)`: both scopes, outermost first.
    let inner2 = schema_of::<Inner2>();
    assert_eq!(
        inner2.get_brand_scopes(),
        &[BrandScope::new(outer_id, 2), BrandScope::new(inner2_id, 1)]
    );
    assert_eq!(
        inner2.get_brand_binding(outer_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        inner2.get_brand_binding(outer_id, 1),
        Some(ty::<data::Owned>())
    );
    assert_eq!(
        inner2.get_brand_binding(inner2_id, 0),
        Some(ty::<test_all_types::Owned>())
    );
    assert_eq!(inner2.get_brand_binding(inner2_id, 1), None);

    // `...Inner2(Baz).DeepNest(Qux)`: three scopes.
    let deep_nest = schema_of::<DeepNest>();
    assert_eq!(
        deep_nest.get_brand_scopes(),
        &[
            BrandScope::new(outer_id, 2),
            BrandScope::new(inner2_id, 1),
            BrandScope::new(deep_nest_id, 1),
        ]
    );
    assert_eq!(
        deep_nest.get_brand_binding(outer_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        deep_nest.get_brand_binding(outer_id, 1),
        Some(ty::<data::Owned>())
    );
    assert_eq!(
        deep_nest.get_brand_binding(inner2_id, 0),
        Some(ty::<test_all_types::Owned>())
    );
    assert_eq!(
        deep_nest.get_brand_binding(deep_nest_id, 0),
        Some(ty::<capnp::primitive_list::Owned<u32>>())
    );

    // `TestGenerics(Foo, Bar).Inner` has no parameters of its own, but binds its parent's.
    let inner = schema_of::<test_generics::inner::Owned<text::Owned, data::Owned>>();
    assert_eq!(inner.get_brand_scopes(), &[BrandScope::new(outer_id, 2)]);
    assert_eq!(
        inner.get_brand_binding(outer_id, 1),
        Some(ty::<data::Owned>())
    );
}

#[test]
fn group_binds_parent_scope() {
    type Parent = test_generics_groups::Owned<text::Owned, data::Owned>;
    let parent = schema_of::<Parent>();
    let parent_id = parent.get_proto().get_id();

    let bar = parent.get_field_by_name("bar").unwrap();
    let group = struct_schema(bar.get_type());
    assert_ne!(group.get_proto().get_id(), parent_id);
    assert_eq!(group.get_brand_scopes(), &[BrandScope::new(parent_id, 2)]);
    assert_eq!(
        group.get_brand_binding(parent_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        group.get_brand_binding(parent_id, 1),
        Some(ty::<data::Owned>())
    );
    // The group declares no scope of its own.
    assert_eq!(group.get_brand_binding(group.get_proto().get_id(), 0), None);

    // The same through the group's own generated type.
    let direct = schema_of::<test_generics_groups::bar::Owned<text::Owned, data::Owned>>();
    assert_eq!(direct, group);
    assert_eq!(
        direct.get_brand_binding(parent_id, 1),
        Some(ty::<data::Owned>())
    );

    // A union group inside a nested generic struct binds all enclosing scopes.
    type Inner = test_generics_groups::inner::Owned<
        text::Owned,
        data::Owned,
        test_all_types::Owned,
        text::Owned,
    >;
    let inner = schema_of::<Inner>();
    let inner_id = inner.get_proto().get_id();
    let union_group = struct_schema(inner.get_field_by_name("bar").unwrap().get_type());
    assert_eq!(
        union_group.get_brand_scopes(),
        &[BrandScope::new(parent_id, 2), BrandScope::new(inner_id, 2)]
    );
    assert_eq!(
        union_group.get_brand_binding(inner_id, 0),
        Some(ty::<test_all_types::Owned>())
    );
}

#[test]
fn phantom_parameter_distinguishes_instantiations() {
    // `TestGenericsGroups.Inner(T3, T4)` never uses `T4` (nor `T1`, `T2`).
    type WithText = test_generics_groups::inner::Owned<
        any_pointer::Owned,
        any_pointer::Owned,
        text::Owned,
        text::Owned,
    >;
    type WithData = test_generics_groups::inner::Owned<
        any_pointer::Owned,
        any_pointer::Owned,
        text::Owned,
        data::Owned,
    >;
    let with_text = schema_of::<WithText>();
    let with_data = schema_of::<WithData>();
    let inner_id = with_text.get_proto().get_id();

    // Same field types...
    let field_types = |s: StructSchema| {
        s.get_fields()
            .unwrap()
            .iter()
            .map(|f| f.get_type())
            .collect::<Vec<_>>()
    };
    assert_eq!(field_types(with_text)[0], field_types(with_data)[0]);

    // ...but different bindings (and unequal schemas).
    assert_eq!(with_text.get_brand_scopes(), with_data.get_brand_scopes());
    assert_eq!(
        with_text.get_brand_binding(inner_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        with_data.get_brand_binding(inner_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        with_text.get_brand_binding(inner_id, 1),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        with_data.get_brand_binding(inner_id, 1),
        Some(ty::<data::Owned>())
    );
    assert_ne!(with_text, with_data);
}

#[test]
fn nested_instantiation() {
    // `TestGenericsWrapper(TestGenerics(Text, Data), TestAllTypes)`
    type Generics = test_generics::Owned<text::Owned, data::Owned>;
    type Wrapper = test_generics_wrapper::Owned<Generics, test_all_types::Owned>;
    let wrapper = schema_of::<Wrapper>();
    let wrapper_id = wrapper.get_proto().get_id();
    let generics_id = id_of::<Generics>();

    let foo = wrapper.get_brand_binding(wrapper_id, 0).unwrap();
    assert_eq!(foo, ty::<Generics>());
    let foo = struct_schema(foo);
    assert_eq!(foo.get_proto().get_id(), generics_id);
    assert_eq!(
        foo.get_brand_binding(generics_id, 0),
        Some(ty::<text::Owned>())
    );
    assert_eq!(
        foo.get_brand_binding(generics_id, 1),
        Some(ty::<data::Owned>())
    );

    // `value :TestGenerics(Foo, Bar)` is `TestGenerics(TestGenerics(Text, Data), TestAllTypes)`:
    // walking its bindings reaches `Text` two levels down.
    let value = struct_schema(wrapper.get_field_by_name("value").unwrap().get_type());
    assert_eq!(value.get_proto().get_id(), generics_id);
    assert_eq!(
        value.get_brand_binding(generics_id, 1),
        Some(ty::<test_all_types::Owned>())
    );
    let value_foo = struct_schema(value.get_brand_binding(generics_id, 0).unwrap());
    assert_eq!(value_foo, foo);
    assert_eq!(
        value_foo.get_brand_binding(generics_id, 0),
        Some(ty::<text::Owned>())
    );
}

#[test]
fn method_results_bind_interface_scope() {
    // `TestGenerics(Foo, Bar).Interface(Qux).call` results `(qux :Qux, ..)`.
    type Results = test_generics::interface::call_results::Owned<
        text::Owned,
        data::Owned,
        test_all_types::Owned,
    >;
    let results = schema_of::<Results>();
    let generics_id = id_of::<test_generics::Owned<text::Owned, data::Owned>>();
    let scopes = results.get_brand_scopes();
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0], BrandScope::new(generics_id, 2));
    let interface_id = scopes[1].scope_id();
    assert_eq!(scopes[1].param_count(), 1);
    assert_ne!(interface_id, results.get_proto().get_id());
    assert_eq!(
        results.get_brand_binding(interface_id, 0),
        Some(ty::<test_all_types::Owned>())
    );
    assert_eq!(
        results.get_field_by_name("qux").unwrap().get_type(),
        ty::<test_all_types::Owned>()
    );
}
