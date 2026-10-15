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

//! `capnp::private::newtype_layout` computed over the raw CodeGeneratorRequest that build.rs
//! dumps gives the same newtype ids and use-site tables as the generated code compiled in. That
//! is what a schema loader using the same functions relies on.

use std::collections::{BTreeMap, HashMap};

use capnp::introspect::{Introspect, TypeVariant};
use capnp::private::newtype_layout::{self, LayoutEntry, LayoutTable, NodeLookup};
use capnp::schema::StructSchema;
use capnp::schema_capnp::{code_generator_request, node};
use capnp::traits::UseSiteTable;
use capnp::{message, serialize};

use crate::test_newtype_capnp::{
    aliases, cross_file, envelope, place_params, place_results, shapes,
};

static CGR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/raw_code_gen_request.bin"));

// `LayoutTable::leak` leaks on purpose. Keeping the result reachable from a static stops Miri
// from reporting it.
static KEPT: std::sync::Mutex<Vec<&'static UseSiteTable<'static>>> =
    std::sync::Mutex::new(Vec::new());

fn leak(table: LayoutTable) -> &'static UseSiteTable<'static> {
    let leaked = table.leak();
    KEPT.lock().unwrap().push(leaked);
    leaked
}

fn read_cgr() -> message::Reader<serialize::OwnedSegments> {
    let mut options = message::ReaderOptions::new();
    options.traversal_limit_in_words(None);
    serialize::read_message(CGR, options).unwrap()
}

fn struct_schema<T: Introspect>() -> StructSchema {
    let TypeVariant::Struct(raw) = T::introspect().which() else {
        panic!("not a struct type");
    };
    StructSchema::new(raw)
}

// Every compiled struct schema reachable from `schema` through struct-typed fields, which
// includes every group, by node id.
fn collect(schema: StructSchema, out: &mut BTreeMap<u64, StructSchema>) {
    let id = schema.get_proto().get_id();
    if out.contains_key(&id) {
        return;
    }
    out.insert(id, schema);
    for field in schema.get_fields().unwrap() {
        if let TypeVariant::Struct(raw) = field.get_type().which() {
            collect(StructSchema::new(raw), out);
        }
    }
}

fn display_name(node: node::Reader<'_>) -> &str {
    node.get_display_name().unwrap().to_str().unwrap()
}

// The file node a node is declared in, and whether it sits inside a newtype's template (an
// ancestor is a `type` node). Template-internal use sites have no generated table: only the
// stamped copies do.
fn file_and_in_template<'a>(nodes: &impl NodeLookup<'a>, node: node::Reader<'a>) -> (u64, bool) {
    let mut in_template = false;
    let mut node = node;
    loop {
        match node.which().unwrap() {
            node::File(()) => return (node.get_id(), in_template),
            node::Type(_) => in_template = true,
            _ => {}
        }
        node = nodes
            .node(node.get_scope_id())
            .expect("scope node in request");
    }
}

#[test]
fn use_site_layouts_match_generated_tables() {
    let message = read_cgr();
    let request: code_generator_request::Reader<'_> = message.get_root().unwrap();
    let mut nodes = HashMap::new();
    for node in request.get_nodes().unwrap() {
        nodes.insert(node.get_id(), node);
    }
    let file_id = request
        .get_requested_files()
        .unwrap()
        .iter()
        .find(|f| f.get_filename().unwrap() == "test-newtype.capnp")
        .expect("test-newtype.capnp in request")
        .get_id();

    // Compiled side: every struct declared in test-newtype.capnp, and everything reachable from
    // them (any missing root shows up as a missing use site below).
    let mut compiled = BTreeMap::new();
    for root in [
        struct_schema::<shapes::Owned>(),
        struct_schema::<cross_file::Owned>(),
        struct_schema::<aliases::Owned>(),
        struct_schema::<place_params::Owned>(),
        struct_schema::<place_results::Owned>(),
        struct_schema::<envelope::Owned<capnp::any_pointer::Owned>>(),
    ] {
        collect(root, &mut compiled);
    }

    // Every compiled struct: the layout computed from its node equals the recorded one, and so
    // do the per-field newtype ids.
    let mut compiled_use_sites = Vec::new();
    for (&id, &schema) in &compiled {
        let node = nodes[&id];
        let computed = newtype_layout::use_site_layout(&nodes, node).unwrap();
        let (recorded_ids, recorded_table) = newtype_layout::recorded_layout(schema);
        match (&computed, recorded_table) {
            (None, None) => assert!(recorded_ids.is_empty(), "{}", display_name(node)),
            (Some(layout), Some(table)) => {
                assert_eq!(layout.newtype_ids, recorded_ids, "{}", display_name(node));
                assert_eq!(layout.table, *table, "{}", display_name(node));
                // And the leaked form is exactly the generated static's value.
                assert_eq!(
                    *leak(layout.table.clone()),
                    *table,
                    "{}",
                    display_name(node)
                );
                compiled_use_sites.push(id);
            }
            _ => panic!(
                "{}: computed {computed:?}, recorded {recorded_table:?}",
                display_name(node)
            ),
        }
        assert_eq!(schema.get_newtype_ids(), recorded_ids);

        let node::Struct(st) = node.which().unwrap() else {
            panic!("not a struct")
        };
        for (field, compiled_field) in st
            .get_fields()
            .unwrap()
            .iter()
            .zip(schema.get_fields().unwrap())
        {
            assert_eq!(
                newtype_layout::field_newtype_ids(&nodes, field).unwrap(),
                compiled_field.get_newtype_ids(),
                "{}.{}",
                display_name(node),
                field.get_name().unwrap().to_str().unwrap()
            );
        }
    }

    // Every use site the request has in test-newtype.capnp (outside templates) was compared.
    let mut request_use_sites = Vec::new();
    for (&id, &node) in &nodes {
        if newtype_layout::use_site_layout(&nodes, node)
            .unwrap()
            .is_none()
        {
            continue;
        }
        if file_and_in_template(&nodes, node) == (file_id, false) {
            request_use_sites.push(id);
        }
    }
    request_use_sites.sort();
    let names = |ids: &[u64]| -> Vec<String> {
        ids.iter()
            .map(|id| display_name(nodes[id]).to_string())
            .collect()
    };
    assert_eq!(names(&request_use_sites), names(&compiled_use_sites));
    // test-newtype.capnp generates 31 `*_TABLE` statics.
    assert_eq!(compiled_use_sites.len(), 31);
}

// Spot checks of the computed tables against the schema text, so the test above isn't
// vacuous if both sides went wrong the same way.
#[test]
fn use_site_layout_examples() {
    let message = read_cgr();
    let request: code_generator_request::Reader<'_> = message.get_root().unwrap();
    let mut nodes = BTreeMap::new();
    for node in request.get_nodes().unwrap() {
        nodes.insert(node.get_id(), node);
    }
    let by_name = |name: &str| {
        *nodes
            .values()
            .find(|n| display_name(**n) == name)
            .unwrap_or_else(|| panic!("no node {name}"))
    };
    let layout = |name: &str| {
        newtype_layout::use_site_layout(&nodes, by_name(name))
            .unwrap()
            .unwrap_or_else(|| panic!("{name} is not a use site"))
    };

    // `bottomRight @[3, 4, 5] :Vec3`: three Float32 slots at data offsets 3..5.
    let bottom_right = layout("test-newtype.capnp:Shapes.bottomRight");
    assert_eq!(
        bottom_right.newtype_ids,
        [by_name("test-newtype.capnp:Vec3").get_id()]
    );
    assert_eq!(
        bottom_right.table.entries,
        [
            LayoutEntry::Offset(3),
            LayoutEntry::Offset(4),
            LayoutEntry::Offset(5)
        ]
    );

    // `pricedPartial @[26] :Priced`: the second member is unmapped.
    let partial = layout("test-newtype.capnp:Shapes.pricedPartial");
    assert_eq!(partial.table.entries.len(), 2);
    assert_eq!(
        partial.table.entries[1],
        LayoutEntry::Offset(capnp::traits::UNMAPPED)
    );

    // `chained @[6-8] :ChainedPoint`: the alias chain, nearest first.
    let chained = layout("test-newtype.capnp:Aliases.chained");
    assert_eq!(
        chained.newtype_ids,
        [
            by_name("test-newtype.capnp:ChainedPoint").get_id(),
            by_name("test-newtype.capnp:Point").get_id(),
            by_name("test-newtype.capnp:Vec3").get_id(),
        ]
    );

    // `prices :OrderPrices`: both members are nested Price use sites with tables of their own.
    let prices = layout("test-newtype.capnp:Shapes.prices");
    assert_eq!(prices.table.entries.len(), 2);
    for (entry, child) in prices.table.entries.iter().zip(["limit", "stop"]) {
        let LayoutEntry::Nested(Some(nested)) = entry else {
            panic!("{child}: {entry:?}");
        };
        let child_node = nodes[&nested.group_id];
        assert_eq!(
            display_name(child_node),
            format!("test-newtype.capnp:Shapes.prices.{child}")
        );
        assert_eq!(nested.table.entries.len(), 2);
        assert!(matches!(nested.table.entries[0], LayoutEntry::Offset(_)));
    }

    // A struct that is not a use site, and a non-group node.
    assert_eq!(
        newtype_layout::use_site_layout(&nodes, by_name("test-newtype.capnp:Shapes")).unwrap(),
        None
    );
    assert_eq!(
        newtype_layout::use_site_layout(&nodes, by_name("test-newtype.capnp:Vec3")).unwrap(),
        None
    );

    // Leaking an unmapped nested member gives the shared empty table.
    let table = LayoutTable {
        entries: vec![LayoutEntry::Nested(None), LayoutEntry::Offset(7)],
        discriminant_offset: 2,
    };
    let leaked = leak(table.clone());
    assert!(std::ptr::eq(
        leaked.nested(0),
        &capnp::traits::EMPTY_USE_SITE_TABLE
    ));
    assert_eq!(leaked.offset(1), 7);
    assert_eq!(leaked.discriminant_offset, 2);
    assert_eq!(table, *leaked);
}
