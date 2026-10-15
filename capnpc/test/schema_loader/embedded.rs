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

//! Generated `SCHEMA_BYTES` (`CompilerCommand::embed_schema`) and `Type::write_schema_type`:
//! what a process needs to send another one, which has no generated code, so that it can read
//! the sender's (branded) types.

// Under Miri (CI's default configuration only), a curated subset runs: the tests marked
// `#[cfg_attr(miri, ignore)]` load large schemas or sweep them, which takes minutes there and
// exercises safe code that native runs cover.

use capnp::introspect::{Introspect, Type, TypeVariant};
use capnp::schema::StructSchema;
use capnp::schema_capnp::{code_generator_request, type_};
use capnp::schema_loader::SchemaLoader;
use capnp::{data, dynamic_struct, message, serialize, text};

use super::id_of;
use crate::test_schema_loader_protocol_capnp::{
    self as protocol, envelope, message as msg, outer, phantom, reply, request, Kind,
};

type MessageRequest = msg::Owned<request::Owned<text::Owned>>;

/// Loads a generated file's `SCHEMA_BYTES`, read in place (they are aligned).
fn load_embedded(loader: &SchemaLoader, mut bytes: &[u8]) -> capnp::Result<()> {
    assert_eq!(
        bytes.as_ptr() as usize % 8,
        0,
        "SCHEMA_BYTES is word-aligned"
    );
    let message = serialize::read_message_from_flat_slice(&mut bytes, Default::default())?;
    let request: code_generator_request::Reader<'_> = message.get_root()?;
    loader.load_code_generator_request(request)
}

/// `ty` as the schema `Type` message a sender would put on the wire.
fn send(ty: Type) -> message::Builder<message::HeapAllocator> {
    let mut message = message::Builder::new_default();
    ty.write_schema_type(message.init_root());
    message
}

/// What a receiver resolves `ty`'s wire form to.
fn receive(loader: &SchemaLoader, ty: Type) -> Type {
    let message = send(ty);
    let wire: type_::Reader<'_> = message.get_root_as_reader().unwrap();
    loader.get_type(wire, None).unwrap()
}

fn struct_schema(ty: Type) -> StructSchema {
    match ty.which() {
        TypeVariant::Struct(raw) => StructSchema::new(raw),
        _ => panic!("not a struct"),
    }
}

fn envelope_field_type(loader: &SchemaLoader, name: &str) -> Type {
    loader
        .get_struct(envelope::TYPE_ID)
        .unwrap()
        .get_field_by_name(name)
        .unwrap()
        .get_type()
}

#[test]
#[cfg_attr(miri, ignore)]
fn each_embedded_schema_loads_on_its_own() {
    // test.capnp imports rust.capnp and another file, uses annotations and interfaces;
    // test-newtype.capnp imports `type` newtypes from test-newtype-import.capnp.
    for (name, bytes) in [
        ("test", crate::test_capnp::SCHEMA_BYTES),
        ("test-newtype", crate::test_newtype_capnp::SCHEMA_BYTES),
        (
            "test-newtype-import",
            crate::test_newtype_import_capnp::SCHEMA_BYTES,
        ),
        (
            "in-submodule",
            crate::foo::bar::in_submodule_capnp::SCHEMA_BYTES,
        ),
        (
            "in-other-submodule",
            crate::baz::in_other_submodule_capnp::SCHEMA_BYTES,
        ),
        ("test-in-dir", crate::test_in_dir_capnp::SCHEMA_BYTES),
        (
            "test-in-src-prefix-dir",
            crate::test_in_src_prefix_dir_capnp::SCHEMA_BYTES,
        ),
        ("protocol", protocol::SCHEMA_BYTES),
    ] {
        load_embedded(&SchemaLoader::new(), bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn embedded_schema_holds_only_the_files_it_reaches() {
    let loader = SchemaLoader::new();
    load_embedded(&loader, crate::test_newtype_import_capnp::SCHEMA_BYTES).unwrap();
    // Compiled in the same run, but not imported by it.
    assert!(loader
        .get_node(crate::test_capnp::test_all_types::TYPE_ID)
        .is_err());
    assert!(loader.get_node(envelope::TYPE_ID).is_err());
}

#[test]
#[cfg_attr(miri, ignore)]
fn embedded_schemas_sharing_imports_load_one_after_another() {
    let loader = SchemaLoader::new();
    load_embedded(&loader, crate::test_newtype_import_capnp::SCHEMA_BYTES).unwrap();
    load_embedded(&loader, crate::test_newtype_capnp::SCHEMA_BYTES).unwrap();
    load_embedded(&loader, crate::test_newtype_capnp::SCHEMA_BYTES).unwrap();
}

#[test]
#[cfg_attr(miri, ignore)]
fn branded_types_resolve_to_the_instances_the_compiler_wrote() {
    let loader = SchemaLoader::new();
    load_embedded(&loader, protocol::SCHEMA_BYTES).unwrap();
    // Each is a field type of Envelope, as `capnp compile` branded it.
    for (field, ty) in [
        ("single", MessageRequest::introspect()),
        ("messages", Type::list_of(MessageRequest::introspect())),
        ("phantom", phantom::Owned::<text::Owned>::introspect()),
        ("kinds", Type::list_of(Kind::introspect())),
        (
            "nested",
            outer::inner::Owned::<text::Owned, data::Owned>::introspect(),
        ),
    ] {
        assert_eq!(
            receive(&loader, ty),
            envelope_field_type(&loader, field),
            "{field}"
        );
    }
    assert_ne!(
        receive(&loader, phantom::Owned::<data::Owned>::introspect()),
        envelope_field_type(&loader, "phantom")
    );
    for ty in [
        text::Owned::introspect(),
        <u32 as Introspect>::introspect(),
        capnp::any_pointer::Owned::introspect(),
    ] {
        assert_eq!(receive(&loader, ty), ty);
    }
}

#[test]
fn received_type_downcasts_to_the_compiled_one() {
    let loader = SchemaLoader::new();
    load_embedded(&loader, protocol::SCHEMA_BYTES).unwrap();
    loader.load_compiled::<MessageRequest>().unwrap();
    let received = receive(&loader, MessageRequest::introspect());
    assert!(received.usable_as(MessageRequest::introspect()));
    assert!(!received.usable_as(msg::Owned::<reply::Owned<text::Owned>>::introspect()));
    assert!(!received.usable_as(msg::Owned::<request::Owned<data::Owned>>::introspect()));
}

#[test]
fn reader_without_generated_code_reads_a_branded_payload() {
    // Sender: a `Message(Request(Text))`, plus its schema and type for the handshake.
    let mut payload = message::Builder::new_default();
    {
        let mut root = payload.init_root::<msg::Builder<'_, request::Owned<text::Owned>>>();
        root.set_id(42);
        root.init_payload().set_body("hello").unwrap();
    }
    let handshake = (
        protocol::SCHEMA_BYTES.to_vec(),
        serialize::write_message_to_words(&send(MessageRequest::introspect())),
    );

    // Receiver: only the handshake and the payload. Bytes off the wire needn't be aligned, so
    // they're read with `read_message`, which copies.
    let loader = SchemaLoader::new();
    let schema_message = serialize::read_message(&handshake.0[..], Default::default()).unwrap();
    loader
        .load_code_generator_request(schema_message.get_root().unwrap())
        .unwrap();
    let wire_type = serialize::read_message(&handshake.1[..], Default::default()).unwrap();
    let ty = loader
        .get_type(wire_type.get_root().unwrap(), None)
        .unwrap();
    let schema = struct_schema(ty);
    assert_eq!(schema.get_proto().get_id(), id_of::<MessageRequest>());

    let root = payload
        .get_root_as_reader::<capnp::any_pointer::Reader<'_>>()
        .unwrap();
    let root = root.get_as_dynamic(schema).unwrap();
    assert_eq!(root.get_named("id").unwrap().downcast::<u32>(), 42);
    let request: dynamic_struct::Reader<'_> = root.get_named("payload").unwrap().downcast();
    // `T` is bound: `body` reads as Text, not AnyPointer.
    let body: text::Reader<'_> = request.get_named("body").unwrap().downcast();
    assert_eq!(body, "hello");
}
