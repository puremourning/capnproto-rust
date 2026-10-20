fn main() {
    capnpc::CompilerCommand::new()
        .crate_provides("external_crate", [0xe6f94f52f7be8fe2])
        .file("test.capnp")
        .file("test-newtype.capnp")
        .file("test-newtype-import.capnp")
        .file("in-submodule.capnp")
        .file("in-other-submodule.capnp")
        .file("schema/test-in-dir.capnp")
        .file("schema-with-src-prefix/test-in-src-prefix-dir.capnp")
        .src_prefix("schema-with-src-prefix")
        .raw_code_generator_request_path(
            std::env::var("OUT_DIR").expect("OUT_DIR env var is not set")
                + "/raw_code_gen_request.bin",
        )
        .run()
        .expect("compiling schema");

    // A file that reaches another crate's annotation only through that crate's newtype. It
    // runs alone: the type names a run can use are shared by all its files, and test.capnp
    // imports the annotation's file directly.
    capnpc::CompilerCommand::new()
        .crate_provides("external_crate", [0xe6f94f52f7be8fe2, 0xd7e863a77bb1f889])
        .file("test-external-newtype.capnp")
        .run()
        .expect("compiling schema");

    // Two versions of an evolving newtype schema, for the cross-version tests in
    // newtype_evolution.rs. Their newtypes share ids, so each needs a run of its own.
    for file in [
        "test-newtype-evolution-v1.capnp",
        "test-newtype-evolution-v2.capnp",
    ] {
        capnpc::CompilerCommand::new()
            .file(file)
            .run()
            .expect("compiling schema");
    }

    capnpc::CompilerCommand::new()
        .file("test-default-parent-module.capnp")
        .file("test-default-parent-module-override.capnp")
        .default_parent_module(vec![
            "test_default_parent_module".into(),
            "test_default_parent_module_inner".into(),
        ])
        .run()
        .expect("compiling schema");

    let mut output_path =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR env var is not set"));

    // `capnp compile` will create this directory
    output_path.push("inner-output-path");
    capnpc::CompilerCommand::new()
        .file("test-output-path.capnp")
        .output_path(output_path)
        .run()
        .expect("compiling schema")
}
