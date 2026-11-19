fn main() {
    // The envelope is compiled into the tool. It imports `/capnp/schema.capnp`, which resolves
    // to the copy at the repository root (`capnp/schema.capnp`); the `capnp` crate already
    // provides generated code for it as `capnp::schema_capnp`.
    ::capnpc::CompilerCommand::new()
        .file("envelope.capnp")
        .import_path("../..")
        .no_standard_import()
        .crate_provides("capnp", [0xa93fc509624c72d9])
        .run()
        .expect("compiling envelope schema");

    // The payload schema is *not* compiled into the tool: we only keep its raw
    // `CodeGeneratorRequest`, which `dynamic-dump write` embeds with `include_bytes!`. capnpc
    // still writes Rust code for it, into a directory that nothing includes.
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    ::capnpc::CompilerCommand::new()
        .file("payload.capnp")
        .output_path(out_dir.join("unused-payload-code"))
        .raw_code_generator_request_path(out_dir.join("payload.cgr"))
        .run()
        .expect("compiling payload schema");
}
