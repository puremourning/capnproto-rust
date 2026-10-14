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

//! Erased newtype carriers (`AnyReader` / `AnyBuilder`) reading and writing a use site whose
//! table was generated from a different version of the newtype. test-newtype-evolution-v1.capnp
//! and -v2.capnp are two versions of the same newtypes (same ids). A use site's table has one
//! entry per field of the newtype's template, by field-list position, with nested newtypes in
//! tables of their own, so a carrier of either version finds the members it knows, reads the
//! ones the table lacks as defaults, and refuses to set them.

use crate::test_newtype_evolution_v1_capnp as v1;
use crate::test_newtype_evolution_v2_capnp as v2;
use capnp::traits::{
    IntoInternalStructBuilder, IntoInternalStructReader, UseSiteEntry, UseSiteTable,
    EMPTY_USE_SITE_TABLE, UNMAPPED,
};
use capnp::{dynamic_struct, dynamic_value, message};

fn v1_message() -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let mut h = message.init_root::<v1::holder::Builder<'_>>();
    h.reborrow().get_a().set_x(1.0);
    h.reborrow().get_a().set_y(2.0);
    h.set_tag(77);
    h.reborrow().get_l().set_id(3);
    h.reborrow().get_l().get_pos().set_x(4.0);
    h.reborrow().get_l().get_pos().set_y(5.0);
    let mut point = h.reborrow().get_s().init_point();
    point.set_x(6.0);
    point.set_y(7.0);
    h.reborrow().get_r().get_tl().set_x(8.0);
    h.reborrow().get_r().get_tl().set_y(9.0);
    h.reborrow().get_r().get_br().set_x(10.0);
    h.reborrow().get_r().get_br().set_y(11.0);
    message
}

fn v2_message() -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let mut h = message.init_root::<v2::holder::Builder<'_>>();
    h.reborrow().get_a().set_x(1.0);
    h.reborrow().get_a().set_y(2.0);
    h.reborrow().get_a().set_z(21.0);
    h.set_tag(77);
    h.reborrow().get_l().set_id(3);
    h.reborrow().get_l().get_pos().set_x(4.0);
    h.reborrow().get_l().get_pos().set_y(5.0);
    h.reborrow().get_l().get_pos().set_z(22.0);
    h.reborrow().get_l().set_name("hello");
    let mut point = h.reborrow().get_s().init_point();
    point.set_x(6.0);
    point.set_y(7.0);
    point.set_z(23.0);
    h.reborrow().get_r().get_tl().set_x(8.0);
    h.reborrow().get_r().get_tl().set_y(9.0);
    h.reborrow().get_r().get_br().set_x(10.0);
    h.reborrow().get_r().get_br().set_y(11.0);
    message
}

fn dynamic_root<'a>(reader: impl Into<dynamic_value::Reader<'a>>) -> dynamic_struct::Reader<'a> {
    reader.into().downcast()
}

// The checks every version-1 carrier passes over a use site holding `v1_message`'s values.
fn check_v1_view(
    a: v1::vec::AnyReader<'_>,
    l: v1::labeled::AnyReader<'_>,
    s: v1::shape::AnyReader<'_>,
    r: v1::span::AnyReader<'_>,
) {
    assert_eq!((a.get_x(), a.get_y()), (1.0, 2.0));
    assert_eq!(l.get_id(), 3);
    assert_eq!((l.get_pos().get_x(), l.get_pos().get_y()), (4.0, 5.0));
    match s.which().unwrap() {
        v1::shape::Which::Point(p) => assert_eq!((p.get_x(), p.get_y()), (6.0, 7.0)),
        _ => panic!("expected point"),
    }
    // Span's `br` comes after a nested member whose newtype gained a field. A single flat table
    // in tree order would shift `br` by the unmapped `tl.z`.
    assert_eq!((r.get_tl().get_x(), r.get_tl().get_y()), (8.0, 9.0));
    assert_eq!((r.get_br().get_x(), r.get_br().get_y()), (10.0, 11.0));
}

#[test]
fn v2_use_site_through_v1_carriers() {
    let message = v2_message();
    let h = message
        .get_root_as_reader::<v2::holder::Reader<'_>>()
        .unwrap();

    // Typed: a version-1 carrier over a version-2 use site's own table.
    check_v1_view(
        v1::vec::AnyReader::new(
            h.get_a().into_internal_struct_reader(),
            &v2::holder::A_TABLE,
        ),
        v1::labeled::AnyReader::new(
            h.get_l().into_internal_struct_reader(),
            &v2::holder::L_TABLE,
        ),
        v1::shape::AnyReader::new(
            h.get_s().into_internal_struct_reader(),
            &v2::holder::S_TABLE,
        ),
        v1::span::AnyReader::new(
            h.get_r().into_internal_struct_reader(),
            &v2::holder::R_TABLE,
        ),
    );

    // Dynamic: the version-2 schema's groups downcast to the version-1 carriers.
    let root = dynamic_root(h);
    let group = |name| root.get_named(name).unwrap();
    check_v1_view(
        group("a").downcast(),
        group("l").downcast(),
        group("s").downcast(),
        group("r").downcast(),
    );
}

#[test]
fn v1_use_site_through_v2_carriers() {
    let message = v1_message();
    let h = message
        .get_root_as_reader::<v1::holder::Reader<'_>>()
        .unwrap();
    let typed = (
        v2::vec::AnyReader::new(
            h.get_a().into_internal_struct_reader(),
            &v1::holder::A_TABLE,
        ),
        v2::labeled::AnyReader::new(
            h.get_l().into_internal_struct_reader(),
            &v1::holder::L_TABLE,
        ),
        v2::shape::AnyReader::new(
            h.get_s().into_internal_struct_reader(),
            &v1::holder::S_TABLE,
        ),
        v2::span::AnyReader::new(
            h.get_r().into_internal_struct_reader(),
            &v1::holder::R_TABLE,
        ),
    );
    let root = dynamic_root(h);
    let group = |name| root.get_named(name).unwrap();
    let dynamic = (
        group("a").downcast(),
        group("l").downcast(),
        group("s").downcast(),
        group("r").downcast(),
    );

    for (a, l, s, r) in [typed, dynamic] {
        let (a, l, s, r): (
            v2::vec::AnyReader<'_>,
            v2::labeled::AnyReader<'_>,
            v2::shape::AnyReader<'_>,
            v2::span::AnyReader<'_>,
        ) = (a, l, s, r);
        // Old fields read as written; fields the table doesn't have read as defaults.
        assert_eq!((a.get_x(), a.get_y(), a.get_z()), (1.0, 2.0, 0.0));
        assert_eq!(l.get_id(), 3);
        let pos = l.get_pos();
        assert_eq!((pos.get_x(), pos.get_y(), pos.get_z()), (4.0, 5.0, 0.0));
        assert!(!l.has_name());
        assert_eq!(l.get_name().unwrap(), "");
        match s.which().unwrap() {
            v2::shape::Which::Point(p) => {
                assert_eq!((p.get_x(), p.get_y(), p.get_z()), (6.0, 7.0, 0.0))
            }
            _ => panic!("expected point"),
        }
        assert_eq!((r.get_tl().get_x(), r.get_tl().get_y()), (8.0, 9.0));
        assert_eq!((r.get_br().get_x(), r.get_br().get_y()), (10.0, 11.0));
    }
}

#[test]
fn v1_carrier_reads_newer_union_arm_as_not_in_schema() {
    let mut message = v2_message();
    message
        .get_root::<v2::holder::Builder<'_>>()
        .unwrap()
        .get_s()
        .set_square(5.0);
    let h = message
        .get_root_as_reader::<v2::holder::Reader<'_>>()
        .unwrap();
    let s: v1::shape::AnyReader<'_> = dynamic_root(h).get_named("s").unwrap().downcast();
    match s.which() {
        Err(capnp::NotInSchema(3)) => {}
        _ => panic!("expected NotInSchema(3)"),
    }
}

#[test]
fn v1_carriers_write_v2_use_site() {
    let mut message = v2_message();
    {
        let root: dynamic_value::Builder<'_> = message
            .get_root::<v2::holder::Builder<'_>>()
            .unwrap()
            .into();
        let mut root: dynamic_struct::Builder<'_> = root.downcast();
        let mut a: v1::vec::AnyBuilder<'_> = root.reborrow().get_named("a").unwrap().downcast();
        a.set_y(-2.0);
        let r: v1::span::AnyBuilder<'_> = root.reborrow().get_named("r").unwrap().downcast();
        r.get_br().set_y(-11.0);
        let s: v1::shape::AnyBuilder<'_> = root.reborrow().get_named("s").unwrap().downcast();
        let mut p = s.init_point();
        p.set_x(-6.0);
    }
    {
        // Typed, through the version-2 use site's own table.
        let h = message.get_root::<v2::holder::Builder<'_>>().unwrap();
        let mut l = v1::labeled::AnyBuilder::new(
            h.get_l().into_internal_struct_builder(),
            &v2::holder::L_TABLE,
        );
        l.set_id(33);
    }
    let h = message
        .get_root_as_reader::<v2::holder::Reader<'_>>()
        .unwrap();
    assert_eq!(
        (h.get_a().get_x(), h.get_a().get_y(), h.get_a().get_z()),
        (1.0, -2.0, 21.0)
    );
    assert_eq!(h.get_l().get_id(), 33);
    assert_eq!(h.get_l().get_pos().get_z(), 22.0);
    assert_eq!(h.get_l().get_name().unwrap(), "hello");
    assert_eq!(h.get_r().get_br().get_x(), 10.0);
    assert_eq!(h.get_r().get_br().get_y(), -11.0);
    match h.get_s().which().unwrap() {
        v2::holder::s::Point(p) => assert_eq!((p.get_x(), p.get_y(), p.get_z()), (-6.0, 7.0, 23.0)),
        _ => panic!("expected point"),
    }
}

// A version-2 builder over a version-1 use site, through the dynamic downcast. `f` gets the
// field's dynamic builder and downcasts it to the carrier under test.
fn v2_builder_over_v1(field: &str, f: impl FnOnce(dynamic_value::Builder<'_>)) {
    let mut message = v1_message();
    let root: dynamic_value::Builder<'_> = message
        .get_root::<v1::holder::Builder<'_>>()
        .unwrap()
        .into();
    let root: dynamic_struct::Builder<'_> = root.downcast();
    f(root.get_named(field).unwrap());
}

#[test]
fn v2_builder_over_v1_sets_known_fields() {
    v2_builder_over_v1("a", |v| {
        let mut a: v2::vec::AnyBuilder<'_> = v.downcast();
        a.set_x(100.0);
        assert_eq!(a.get_z(), 0.0); // unmapped: reads the default
    });
}

#[test]
#[should_panic(expected = "field 'z' is not mapped at this use site")]
fn v2_builder_over_v1_set_new_slot_panics() {
    v2_builder_over_v1("a", |v| {
        let mut a: v2::vec::AnyBuilder<'_> = v.downcast();
        a.set_z(1.0);
    });
}

#[test]
#[should_panic(expected = "field 'name' is not mapped at this use site")]
fn v2_builder_over_v1_set_new_pointer_panics() {
    v2_builder_over_v1("l", |v| {
        let mut l: v2::labeled::AnyBuilder<'_> = v.downcast();
        assert!(!l.has_name());
        l.set_name("x");
    });
}

#[test]
#[should_panic(expected = "field 'z' is not mapped at this use site")]
fn v2_builder_over_v1_set_new_nested_slot_panics() {
    v2_builder_over_v1("l", |v| {
        let l: v2::labeled::AnyBuilder<'_> = v.downcast();
        l.get_pos().set_z(1.0);
    });
}

#[test]
#[should_panic(expected = "field 'square' is not mapped at this use site")]
fn v2_builder_over_v1_set_new_union_arm_panics() {
    v2_builder_over_v1("s", |v| {
        let mut s: v2::shape::AnyBuilder<'_> = v.downcast();
        s.set_square(1.0);
    });
}

#[test]
#[should_panic(expected = "field 'z' is not mapped at this use site")]
fn v2_builder_over_v1_set_new_slot_in_union_arm_panics() {
    v2_builder_over_v1("s", |v| {
        let s: v2::shape::AnyBuilder<'_> = v.downcast();
        let mut p = s.init_point();
        p.set_x(1.0);
        p.set_z(1.0);
    });
}

#[test]
#[should_panic(expected = "field 'z' is not mapped at this use site")]
fn v2_builder_over_v1_typed_table_set_panics() {
    let mut message = v1_message();
    let h = message.get_root::<v1::holder::Builder<'_>>().unwrap();
    let mut a = v2::vec::AnyBuilder::new(
        h.get_a().into_internal_struct_builder(),
        &v1::holder::A_TABLE,
    );
    a.set_z(1.0);
}

// Tables built by hand, as a schema loaded at runtime would build them.
#[test]
fn hand_built_tables() {
    let message = v2_message();
    let h = message
        .get_root_as_reader::<v2::holder::Reader<'_>>()
        .unwrap();
    let raw = h.get_r().into_internal_struct_reader();

    // Longer than the carrier knows: the extra entries are ignored.
    static TL: UseSiteTable<'static> = UseSiteTable {
        entries: &[
            UseSiteEntry::Offset(10),
            UseSiteEntry::Offset(11),
            UseSiteEntry::Offset(UNMAPPED),
            UseSiteEntry::Offset(0),
        ],
        discriminant_offset: 0,
    };
    // Shorter: `y` reads as its default.
    static BR: UseSiteTable<'static> = UseSiteTable {
        entries: &[UseSiteEntry::Offset(12)],
        discriminant_offset: 0,
    };
    static R: UseSiteTable<'static> = UseSiteTable {
        entries: &[UseSiteEntry::Nested(&TL), UseSiteEntry::Nested(&BR)],
        discriminant_offset: 0,
    };
    // Span's offsets in units of Float32: tl = 9, 10; br = 11, 12. Swap them so the reads show
    // which entry each member came from.
    static SWAPPED_TL: UseSiteTable<'static> = UseSiteTable {
        entries: &[UseSiteEntry::Offset(10), UseSiteEntry::Offset(9)],
        discriminant_offset: 0,
    };
    static SWAPPED: UseSiteTable<'static> = UseSiteTable {
        entries: &[UseSiteEntry::Nested(&SWAPPED_TL)],
        discriminant_offset: 0,
    };
    let rect = v1::span::AnyReader::new(raw, &SWAPPED);
    assert_eq!((rect.get_tl().get_x(), rect.get_tl().get_y()), (9.0, 8.0));
    // `br` is past the end of the table: unmapped, so it reads defaults.
    assert_eq!((rect.get_br().get_x(), rect.get_br().get_y()), (0.0, 0.0));

    let rect = v1::span::AnyReader::new(raw, &R);
    assert_eq!((rect.get_tl().get_x(), rect.get_tl().get_y()), (9.0, 10.0));
    assert_eq!((rect.get_br().get_x(), rect.get_br().get_y()), (11.0, 0.0));

    // An entry of the wrong kind is unmapped too.
    static WRONG_KIND: UseSiteTable<'static> = UseSiteTable {
        entries: &[
            UseSiteEntry::Offset(9),
            UseSiteEntry::Nested(&EMPTY_USE_SITE_TABLE),
        ],
        discriminant_offset: 0,
    };
    let rect = v1::span::AnyReader::new(raw, &WRONG_KIND);
    assert_eq!(rect.get_tl().get_x(), 0.0);
    assert_eq!(rect.get_br().get_x(), 0.0);
    let vec = v1::vec::AnyReader::new(raw, &WRONG_KIND);
    assert_eq!((vec.get_x(), vec.get_y()), (8.0, 0.0));

    // An unmapped union reads as its first arm.
    let s = v2::shape::AnyReader::new(raw, &EMPTY_USE_SITE_TABLE);
    assert!(matches!(s.which(), Ok(v2::shape::Which::None(()))));
}

#[test]
#[should_panic(expected = "field 'x' is not mapped at this use site")]
fn hand_built_empty_table_set_panics() {
    let mut message = v2_message();
    let h = message.get_root::<v2::holder::Builder<'_>>().unwrap();
    let mut a = v2::vec::AnyBuilder::new(
        h.get_a().into_internal_struct_builder(),
        &EMPTY_USE_SITE_TABLE,
    );
    a.set_x(1.0);
}

#[test]
#[should_panic(expected = "field 'point' is not mapped at this use site")]
fn hand_built_unmapped_union_group_arm_init_panics() {
    let mut message = v2_message();
    let h = message.get_root::<v2::holder::Builder<'_>>().unwrap();
    let s = v2::shape::AnyBuilder::new(
        h.get_s().into_internal_struct_builder(),
        &EMPTY_USE_SITE_TABLE,
    );
    s.init_point();
}

// The Vec3 / Rect / S repro from plans/newtype-evolution-fixes.md. Version 2's Vec3 gains `w`; Rect
// maps it at Rect ordinals 6 and 7 and S maps those at S ordinals 7 and 8, so S's v2 use site puts
// tl at S 0,1,2,7 and br at S 3,4,5,8. A flat tree-order table, [0,1,2,7,3,4,5,8], would have a
// v1 `br` carrier (slice 3..6) read tl.w, br.x and br.y.

fn s_v1_message() -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let mut s = message.init_root::<v1::s::Builder<'_>>();
    let mut tl = s.reborrow().get_r().get_tl();
    tl.set_x(1.0);
    tl.set_y(2.0);
    tl.set_z(3.0);
    let mut br = s.reborrow().get_r().get_br();
    br.set_x(4.0);
    br.set_y(5.0);
    br.set_z(6.0);
    s.set_tag(99);
    message
}

fn s_v2_message() -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let mut s = message.init_root::<v2::s::Builder<'_>>();
    let mut tl = s.reborrow().get_r().get_tl();
    tl.set_x(1.0);
    tl.set_y(2.0);
    tl.set_z(3.0);
    tl.set_w(7.0);
    let mut br = s.reborrow().get_r().get_br();
    br.set_x(4.0);
    br.set_y(5.0);
    br.set_z(6.0);
    br.set_w(8.0);
    s.set_tag(99);
    message
}

fn check_rect_v1(r: v1::rect::AnyReader<'_>) {
    let (tl, br) = (r.get_tl(), r.get_br());
    assert_eq!((tl.get_x(), tl.get_y(), tl.get_z()), (1.0, 2.0, 3.0));
    assert_eq!((br.get_x(), br.get_y(), br.get_z()), (4.0, 5.0, 6.0));
}

#[test]
fn rect_v2_use_site_through_v1_carriers() {
    let message = s_v2_message();
    let s = message.get_root_as_reader::<v2::s::Reader<'_>>().unwrap();
    // The v2 use site's own carrier, from `as_any()`, sees w.
    let own = s.get_r().as_any();
    assert_eq!((own.get_tl().get_w(), own.get_br().get_w()), (7.0, 8.0));
    // A v1 carrier over the same table `as_any()` uses.
    check_rect_v1(v1::rect::AnyReader::new(
        s.get_r().into_internal_struct_reader(),
        &v2::s::R_TABLE,
    ));
    // The nested use sites have tables of their own.
    let br = v1::vec3::AnyReader::new(s.get_r().into_internal_struct_reader(), &v2::s::r::BR_TABLE);
    assert_eq!((br.get_x(), br.get_y(), br.get_z()), (4.0, 5.0, 6.0));
    // Dynamic: S's v2 schema downcast to the v1 carriers, at the top and nested levels.
    let root = dynamic_root(s);
    check_rect_v1(root.get_named("r").unwrap().downcast());
    let dynamic_value::Reader::Struct(r) = root.get_named("r").unwrap() else {
        panic!("not a group");
    };
    let br: v1::vec3::AnyReader<'_> = r.get_named("br").unwrap().downcast();
    assert_eq!((br.get_x(), br.get_y(), br.get_z()), (4.0, 5.0, 6.0));
    assert_eq!(s.get_tag(), 99);
}

#[test]
fn rect_v1_use_site_through_v2_carriers() {
    let message = s_v1_message();
    let s = message.get_root_as_reader::<v1::s::Reader<'_>>().unwrap();
    let typed = v2::rect::AnyReader::new(s.get_r().into_internal_struct_reader(), &v1::s::R_TABLE);
    let dynamic: v2::rect::AnyReader<'_> = dynamic_root(s).get_named("r").unwrap().downcast();
    for r in [typed, dynamic] {
        let (tl, br) = (r.get_tl(), r.get_br());
        // w is absent from the v1 tables: it reads as its default.
        assert_eq!(
            (tl.get_x(), tl.get_y(), tl.get_z(), tl.get_w()),
            (1.0, 2.0, 3.0, 0.0)
        );
        assert_eq!(
            (br.get_x(), br.get_y(), br.get_z(), br.get_w()),
            (4.0, 5.0, 6.0, 0.0)
        );
    }
}

#[test]
fn rect_v1_builder_writes_v2_use_site() {
    let mut message = s_v2_message();
    {
        let s = message.get_root::<v2::s::Builder<'_>>().unwrap();
        let r =
            v1::rect::AnyBuilder::new(s.get_r().into_internal_struct_builder(), &v2::s::R_TABLE);
        r.get_br().set_x(40.0);
    }
    {
        let root: dynamic_value::Builder<'_> =
            message.get_root::<v2::s::Builder<'_>>().unwrap().into();
        let root: dynamic_struct::Builder<'_> = root.downcast();
        let r: v1::rect::AnyBuilder<'_> = root.get_named("r").unwrap().downcast();
        r.get_tl().set_z(30.0);
    }
    let s = message.get_root_as_reader::<v2::s::Reader<'_>>().unwrap();
    let (tl, br) = (s.get_r().get_tl(), s.get_r().get_br());
    assert_eq!(
        (tl.get_x(), tl.get_y(), tl.get_z(), tl.get_w()),
        (1.0, 2.0, 30.0, 7.0)
    );
    assert_eq!(
        (br.get_x(), br.get_y(), br.get_z(), br.get_w()),
        (40.0, 5.0, 6.0, 8.0)
    );
    assert_eq!(s.get_tag(), 99);
}

#[test]
fn rect_v2_builder_writes_v1_use_site() {
    let mut message = s_v1_message();
    {
        let root: dynamic_value::Builder<'_> =
            message.get_root::<v1::s::Builder<'_>>().unwrap().into();
        let root: dynamic_struct::Builder<'_> = root.downcast();
        let r: v2::rect::AnyBuilder<'_> = root.get_named("r").unwrap().downcast();
        let mut br = r.get_br();
        br.set_y(50.0);
    }
    let s = message.get_root_as_reader::<v1::s::Reader<'_>>().unwrap();
    assert_eq!(s.get_r().get_br().get_y(), 50.0);
    assert_eq!(s.get_r().get_tl().get_x(), 1.0);
    assert_eq!(s.get_tag(), 99);
}

#[test]
#[should_panic(expected = "field 'w' is not mapped at this use site")]
fn rect_v2_builder_over_v1_set_w_panics() {
    let mut message = s_v1_message();
    let s = message.get_root::<v1::s::Builder<'_>>().unwrap();
    let r = v2::rect::AnyBuilder::new(s.get_r().into_internal_struct_builder(), &v1::s::R_TABLE);
    r.get_br().set_w(1.0);
}

#[test]
#[should_panic(expected = "field 'w' is not mapped at this use site")]
fn rect_v2_builder_over_v1_set_w_panics_dynamic() {
    let mut message = s_v1_message();
    let root: dynamic_value::Builder<'_> = message.get_root::<v1::s::Builder<'_>>().unwrap().into();
    let root: dynamic_struct::Builder<'_> = root.downcast();
    let r: v2::rect::AnyBuilder<'_> = root.get_named("r").unwrap().downcast();
    r.get_tl().set_w(1.0);
}
