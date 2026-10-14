@0xb23ce1ba6549c7f1;

using Schema = import "/capnp/schema.capnp";

# A self-describing message: a payload of a type the reader need not know at compile time.
struct Envelope {
  typeId @0 :UInt64;
  # Node id of the payload's struct type.

  payload @1 :AnyPointer;
  # The payload struct.

  schema @2 :List(Schema.Node);
  # Optional: the nodes needed to interpret `payload` (for example, the `nodes` of the
  # `CodeGeneratorRequest` from `capnp compile -o- payload.capnp`). When empty, the reader
  # needs the schema from somewhere else.
}
