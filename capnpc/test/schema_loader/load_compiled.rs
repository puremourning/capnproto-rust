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

//! `SchemaLoader::load_compiled`, `Type::usable_as` and `try_downcast`: loaded values downcast to
//! compiled types once the loader knows them, and only to the exact instantiation.
//!
//! Every test uses a fresh loader, because `load_compiled` changes what downcasts accept.

// Under Miri (CI's default configuration only), a curated subset runs: the tests marked
// `#[cfg_attr(miri, ignore)]` load large schemas or sweep them, which takes minutes there and
// exercises safe code that native runs cover.

use capnp::introspect::{Introspect, TypeVariant};
use capnp::schema_capnp::{code_generator_request, node};
use capnp::schema_loader::SchemaLoader;
use capnp::{data, dynamic_list, dynamic_struct, enum_list, message, struct_list, text};

use super::{id_of, read_cgr, root_pointer};
use crate::test_newtype_evolution_v1_capnp as v1;
use crate::test_schema_loader_protocol_capnp::{
    chain, envelope, message as msg, outer, phantom, reply, request, uses_envelope, Kind,
};

static PROTOCOL_CGR: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/schema-loader-protocol.cgr"));

type MessageRequest = msg::Owned<request::Owned<text::Owned>>;
type MessageReply = msg::Owned<reply::Owned<text::Owned>>;

fn protocol_loader() -> SchemaLoader {
    let loader = SchemaLoader::new();
    let message = read_cgr(PROTOCOL_CGR);
    let request: code_generator_request::Reader<'_> = message.get_root().unwrap();
    loader.load_code_generator_request(request).unwrap();
    loader
}

/// An `Envelope` written through generated code.
fn envelope_message() -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    let mut root = message.init_root::<envelope::Builder<'_>>();
    {
        let mut single = root.reborrow().init_single();
        single.set_id(7);
        single.reborrow().init_payload().set_body("hello").unwrap();
        let mut meta = single.reborrow().init_meta();
        meta.set_tag("t");
        meta.set_kind(Kind::Pong);
        single.init_body().init_other().set_body("other").unwrap();
    }
    {
        let mut messages = root.reborrow().init_messages(2);
        for i in 0..2 {
            let mut m = messages.reborrow().get(i);
            m.set_id(10 + i);
            m.init_payload().set_body("in a list").unwrap();
        }
    }
    root.reborrow().init_phantom().set_value(3);
    {
        let mut kinds = root.reborrow().init_kinds(2);
        kinds.set(0, Kind::Pong);
        kinds.set(1, Kind::Ping);
    }
    {
        let mut nested = root.reborrow().init_nested();
        nested.set_t("t").unwrap();
        nested.set_u(&[1, 2, 3][..]).unwrap();
    }
    {
        let mut chain = root.init_chain();
        chain.set_label("first");
        chain.init_next().set_label("second");
    }
    message
}

fn loaded_envelope<'a, A: message::Allocator>(
    loader: &SchemaLoader,
    message: &'a message::Builder<A>,
) -> dynamic_struct::Reader<'a> {
    root_pointer(message)
        .get_as_dynamic(loader.get_struct(id_of::<envelope::Owned>()).unwrap())
        .unwrap()
}

fn field<'a>(s: dynamic_struct::Reader<'a>, name: &str) -> dynamic_struct::Reader<'a> {
    s.get_named(name).unwrap().downcast()
}

#[test]
fn message_request_downcasts_but_message_reply_does_not() {
    let loader = protocol_loader();
    loader.load_compiled::<MessageRequest>().unwrap();
    loader.load_compiled::<MessageReply>().unwrap();
    let message = envelope_message();
    // A loaded struct reached as a field of another loaded struct.
    let single = field(loaded_envelope(&loader, &message), "single");

    let typed = single.try_downcast::<MessageRequest>().unwrap();
    assert_eq!(typed.get_id(), 7);
    assert_eq!(typed.get_payload().unwrap().get_body().unwrap(), "hello");
    assert_eq!(single.downcast::<MessageRequest>().get_id(), 7);

    // Same node, compatible, but bound to a different type.
    assert!(single.try_downcast::<MessageReply>().is_err());
    assert!(single
        .try_downcast::<msg::Owned<request::Owned<data::Owned>>>()
        .is_err());
    // Through dynamic_value, too.
    let value = loaded_envelope(&loader, &message)
        .get_named("single")
        .unwrap();
    assert!(value.try_downcast_struct::<MessageRequest>().is_ok());
    assert!(value.try_downcast_struct::<MessageReply>().is_err());
    assert!(loaded_envelope(&loader, &message)
        .get_named("phantom")
        .unwrap()
        .try_downcast_struct::<MessageRequest>()
        .is_err());

    // `usable_as` is the relation behind it, and only goes from loaded to compiled.
    let loaded_type: capnp::introspect::Type =
        TypeVariant::Struct(single.get_schema().into()).into();
    assert!(loaded_type.usable_as(MessageRequest::introspect()));
    assert!(!loaded_type.usable_as(MessageReply::introspect()));
    assert!(!MessageRequest::introspect().usable_as(loaded_type));
}

#[test]
#[should_panic(expected = "not usable as the target type")]
#[cfg_attr(miri, ignore)]
fn message_request_does_not_downcast_to_message_reply() {
    let loader = protocol_loader();
    loader.load_compiled::<MessageRequest>().unwrap();
    loader.load_compiled::<MessageReply>().unwrap();
    let message = envelope_message();
    let _ = field(loaded_envelope(&loader, &message), "single").downcast::<MessageReply>();
}

#[test]
#[cfg_attr(miri, ignore)]
fn without_load_compiled_downcasts_fail() {
    let loader = protocol_loader();
    let message = envelope_message();
    let root = loaded_envelope(&loader, &message);
    assert!(root.try_downcast::<envelope::Owned>().is_err());
    assert!(field(root, "single")
        .try_downcast::<MessageRequest>()
        .is_err());
    // Loading one type doesn't make the others usable.
    loader.load_compiled::<MessageRequest>().unwrap();
    assert!(root.try_downcast::<envelope::Owned>().is_err());
    assert!(field(root, "single")
        .try_downcast::<MessageRequest>()
        .is_ok());
}

#[test]
#[should_panic(expected = "not usable as the target type")]
#[cfg_attr(miri, ignore)]
fn without_load_compiled_downcast_panics() {
    let loader = protocol_loader();
    let message = envelope_message();
    let _ = field(loaded_envelope(&loader, &message), "single").downcast::<MessageRequest>();
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_parameter_mismatch_is_rejected() {
    let loader = protocol_loader();
    loader
        .load_compiled::<phantom::Owned<text::Owned>>()
        .unwrap();
    loader
        .load_compiled::<phantom::Owned<data::Owned>>()
        .unwrap();
    let message = envelope_message();
    let p = field(loaded_envelope(&loader, &message), "phantom");
    assert_eq!(
        p.try_downcast::<phantom::Owned<text::Owned>>()
            .unwrap()
            .get_value(),
        3
    );
    assert!(p.try_downcast::<phantom::Owned<data::Owned>>().is_err());
}

#[test]
#[cfg_attr(miri, ignore)]
fn groups_downcast_after_loading_the_parent() {
    let loader = protocol_loader();
    let message = envelope_message();
    let single = field(loaded_envelope(&loader, &message), "single");
    let meta = field(single, "meta");
    let body = field(single, "body");
    assert!(meta
        .try_downcast::<msg::meta::Owned<request::Owned<text::Owned>>>()
        .is_err());

    loader.load_compiled::<MessageRequest>().unwrap();
    let meta = meta
        .try_downcast::<msg::meta::Owned<request::Owned<text::Owned>>>()
        .unwrap();
    assert_eq!(meta.get_tag().unwrap(), "t");
    assert_eq!(meta.get_kind().unwrap(), Kind::Pong);
    // The group takes its parent's bindings, so a different parent instance is rejected.
    assert!(field(single, "meta")
        .try_downcast::<msg::meta::Owned<reply::Owned<text::Owned>>>()
        .is_err());
    let body = body
        .try_downcast::<msg::body::Owned<request::Owned<text::Owned>>>()
        .unwrap();
    let msg::body::Other(other) = body.which().unwrap() else {
        panic!("expected `other`");
    };
    assert_eq!(other.unwrap().get_body().unwrap(), "other");
}

#[test]
fn nested_generic_scopes() {
    let loader = protocol_loader();
    loader.load_compiled::<envelope::Owned>().unwrap();
    let message = envelope_message();
    let nested = field(loaded_envelope(&loader, &message), "nested");
    let typed = nested
        .try_downcast::<outer::inner::Owned<text::Owned, data::Owned>>()
        .unwrap();
    assert_eq!(typed.get_t().unwrap(), "t");
    assert_eq!(typed.get_u().unwrap(), &[1, 2, 3]);
    assert!(nested
        .try_downcast::<outer::inner::Owned<data::Owned, text::Owned>>()
        .is_err());
    assert!(nested
        .try_downcast::<outer::inner::Owned<text::Owned, text::Owned>>()
        .is_err());
}

#[test]
#[cfg_attr(miri, ignore)]
fn lists_of_generic_structs_and_enums_downcast() {
    let loader = protocol_loader();
    let message = envelope_message();
    let messages: dynamic_list::Reader<'_> = loaded_envelope(&loader, &message)
        .get_named("messages")
        .unwrap()
        .downcast();
    assert!(!messages
        .element_type()
        .usable_as(MessageRequest::introspect()));

    loader.load_compiled::<envelope::Owned>().unwrap();
    let root = loaded_envelope(&loader, &message);
    let messages: struct_list::Reader<'_, MessageRequest> =
        root.get_named("messages").unwrap().downcast();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages.get(1).get_id(), 11);
    assert_eq!(
        messages.get(0).get_payload().unwrap().get_body().unwrap(),
        "in a list"
    );
    let dynamic: dynamic_list::Reader<'_> = root.get_named("messages").unwrap().downcast();
    let element_list = capnp::introspect::Type::list_of(dynamic.element_type());
    assert!(element_list.usable_as(struct_list::Owned::<MessageRequest>::introspect()));
    assert!(!element_list.usable_as(struct_list::Owned::<MessageReply>::introspect()));

    let kinds: enum_list::Reader<'_, Kind> = root.get_named("kinds").unwrap().downcast();
    assert_eq!(kinds.get(0).unwrap(), Kind::Pong);
}

#[test]
#[should_panic]
#[cfg_attr(miri, ignore)]
fn list_of_message_request_does_not_downcast_to_message_reply() {
    let loader = protocol_loader();
    loader.load_compiled::<envelope::Owned>().unwrap();
    loader.load_compiled::<MessageReply>().unwrap();
    let message = envelope_message();
    let _: struct_list::Reader<'_, MessageReply> = loaded_envelope(&loader, &message)
        .get_named("messages")
        .unwrap()
        .downcast();
}

#[test]
fn recursive_types() {
    let loader = protocol_loader();
    loader.load_compiled::<chain::Owned>().unwrap();
    let message = envelope_message();
    let first = field(loaded_envelope(&loader, &message), "chain");
    let next = field(first, "next");
    assert_eq!(
        next.try_downcast::<chain::Owned>()
            .unwrap()
            .get_label()
            .unwrap(),
        "second"
    );
}

#[test]
fn loaded_builder_downcasts_to_compiled_builder() {
    let loader = protocol_loader();
    loader.load_compiled::<envelope::Owned>().unwrap();
    let schema = loader.get_struct(id_of::<envelope::Owned>()).unwrap();
    let mut message = message::Builder::new_default();
    {
        let mut root = message.init_root_dynamic(schema).unwrap();
        {
            let mut single: dynamic_struct::Builder<'_> =
                root.reborrow().init_named("single").unwrap().downcast();
            assert!(single.reborrow().try_downcast::<MessageReply>().is_err());
            let mut single = single.try_downcast::<MessageRequest>().unwrap();
            single.set_id(99);
            single.init_payload().set_body("written").unwrap();
        }
        let mut typed = root.try_downcast::<envelope::Owned>().unwrap();
        let messages = typed.reborrow().init_messages(1);
        let dynamic: capnp::dynamic_value::Builder<'_> = messages.into();
        let mut messages: struct_list::Builder<'_, MessageRequest> = dynamic.downcast();
        messages.reborrow().get(0).set_id(5);
        typed.init_phantom().set_value(42);
    }
    let root = message
        .get_root_as_reader::<envelope::Reader<'_>>()
        .unwrap();
    let single = root.get_single().unwrap();
    assert_eq!(single.get_id(), 99);
    assert_eq!(single.get_payload().unwrap().get_body().unwrap(), "written");
    assert_eq!(root.get_messages().unwrap().get(0).get_id(), 5);
    assert_eq!(root.get_phantom().unwrap().get_value(), 42);
}

#[test]
#[cfg_attr(miri, ignore)]
fn mismatched_node_is_an_error() {
    // The same `Request` node, with a field renamed: same id, different content.
    let cgr = read_cgr(PROTOCOL_CGR);
    let request: code_generator_request::Reader<'_> = cgr.get_root().unwrap();
    let mut copy = message::Builder::new_default();
    copy.set_root(request.get_nodes().unwrap()).unwrap();
    let mut nodes = copy
        .get_root::<struct_list::Builder<'_, node::Owned>>()
        .unwrap();
    let request_id = id_of::<request::Owned<text::Owned>>();
    let mut found = false;
    for i in 0..nodes.len() {
        let mut n = nodes.reborrow().get(i);
        if n.reborrow().get_id() == request_id {
            let node::Which::Struct(st) = n.which().unwrap() else {
                panic!("not a struct");
            };
            st.get_fields().unwrap().get(0).set_name("renamed");
            found = true;
        }
    }
    assert!(found);
    let loader = SchemaLoader::new();
    loader.load(nodes.into_reader()).unwrap();

    let err = loader.load_compiled::<MessageRequest>().unwrap_err();
    assert!(err.to_string().contains("not compatible"), "{err}");
    // Nothing changed: unrelated nodes the walk reached were not made usable.
    let message = envelope_message();
    let single = field(loaded_envelope(&loader, &message), "single");
    assert!(field(single, "meta")
        .try_downcast::<msg::meta::Owned<request::Owned<text::Owned>>>()
        .is_err());
    // Types that don't reach `Request` still load.
    loader.load_compiled::<chain::Owned>().unwrap();
}

#[test]
#[cfg_attr(miri, ignore)]
fn compiled_nodes_are_loaded_when_missing() {
    let loader = SchemaLoader::new();
    loader.load_compiled::<envelope::Owned>().unwrap();
    loader.load_compiled::<MessageReply>().unwrap();
    // `UsesEnvelope` alone refers to nodes only load_compiled supplied.
    let cgr = read_cgr(PROTOCOL_CGR);
    let request: code_generator_request::Reader<'_> = cgr.get_root().unwrap();
    let uses_id = id_of::<uses_envelope::Owned>();
    let uses = request
        .get_nodes()
        .unwrap()
        .iter()
        .find(|n| n.get_id() == uses_id)
        .unwrap();
    loader.load([uses]).unwrap();

    let mut message = message::Builder::new_default();
    {
        let mut root = message.init_root::<uses_envelope::Builder<'_>>();
        root.reborrow()
            .init_envelope()
            .init_single()
            .init_payload()
            .set_body("deep")
            .unwrap();
        let mut replies = root.init_replies(1);
        replies.reborrow().get(0).init_payload().set_ok(true);
    }
    let schema = loader.get_struct(uses_id).unwrap();
    let root = root_pointer(&message).get_as_dynamic(schema).unwrap();
    let compiled_root = root_pointer(&message)
        .get_as_dynamic(super::compiled::<uses_envelope::Owned>())
        .unwrap();
    assert_eq!(format!("{root:?}"), format!("{compiled_root:?}"));
    let envelope = field(root, "envelope")
        .try_downcast::<envelope::Owned>()
        .unwrap();
    assert_eq!(
        envelope
            .get_single()
            .unwrap()
            .get_payload()
            .unwrap()
            .get_body()
            .unwrap(),
        "deep"
    );
    let replies: struct_list::Reader<'_, MessageReply> =
        root.get_named("replies").unwrap().downcast();
    assert!(replies.get(0).get_payload().unwrap().get_ok());

    // The whole request still loads on top: the compiled nodes are identical to its own.
    loader.load_code_generator_request(request).unwrap();
}

#[test]
#[cfg_attr(miri, ignore)]
fn compiled_nodes_referring_to_unloaded_nodes_fail_to_load() {
    // TestUseGenerics carries an annotation, whose node generated code doesn't have.
    use crate::test_capnp::test_use_generics;
    let loader = SchemaLoader::new();
    let err = loader
        .load_compiled::<test_use_generics::Owned>()
        .unwrap_err();
    assert!(err.to_string().contains("not loaded"), "{err}");
    assert!(loader
        .get_struct(id_of::<test_use_generics::Owned>())
        .is_err());
}

#[test]
fn newtype_use_site_groups_downcast() {
    let loader = SchemaLoader::new();
    super::load_into(&loader, super::EVOLUTION_V1_CGR).unwrap();
    let message = crate::newtype_evolution::v1_message();
    let root = super::evolution_root(&loader, &message, id_of::<v1::holder::Owned>());
    assert!(field(root, "a")
        .try_downcast::<v1::holder::a::Owned>()
        .is_err());

    loader.load_compiled::<v1::holder::Owned>().unwrap();
    let holder = root.try_downcast::<v1::holder::Owned>().unwrap();
    let a = field(root, "a")
        .try_downcast::<v1::holder::a::Owned>()
        .unwrap();
    assert_eq!(a.get_x(), holder.get_a().get_x());
    assert!(field(root, "l")
        .try_downcast::<v1::holder::a::Owned>()
        .is_err());
    // The newtype downcast is by id and doesn't need load_compiled; both work.
    let _: v1::vec::AnyReader<'_> = root.get_named("a").unwrap().downcast();
}
