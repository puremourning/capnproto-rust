# dynamic-dump example

Prints a message whose payload type is not compiled into the program, using
`capnp::schema_loader::SchemaLoader` and the dynamic API.

Only `envelope.capnp` is compiled into the tool:

```capnp
struct Envelope {
  typeId @0 :UInt64;                 # node id of the payload struct
  payload @1 :AnyPointer;
  schema @2 :List(Schema.Node);      # optional: makes the message self-describing
}
```

`payload.capnp` is not: `build.rs` only keeps its `CodeGeneratorRequest`, which `write` uses to
build a sample `SensorReport` through the dynamic builder API. `read` gets the schema at run
time, from the envelope or from a `CodeGeneratorRequest` file.

Self-describing (the schema travels in the envelope):

```
$ cargo run -p dynamic-dump write | cargo run -p dynamic-dump read
# payload.capnp:SensorReport (0xbf9f0ae6d6b9c488)
(
  station = "Hilltop",
  takenAt = 1760000000000,
  status = degraded,
  readings = [
  ...
```

Schema from a file:

```
$ capnp compile -o- example/dynamic-dump/payload.capnp > payload.cgr
$ cargo run -p dynamic-dump write --no-schema | cargo run -p dynamic-dump read payload.cgr
```

Without either, `read` reports which type id it could not find.
