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
        .embed_schema(true)
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
    // Their raw requests are dumped for the schema loader's cross-version tests.
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR env var is not set");
    for (file, cgr) in [
        ("test-newtype-evolution-v1.capnp", "evolution-v1.cgr"),
        ("test-newtype-evolution-v2.capnp", "evolution-v2.cgr"),
    ] {
        capnpc::CompilerCommand::new()
            .file(file)
            .raw_code_generator_request_path(format!("{out_dir}/{cgr}"))
            .run()
            .expect("compiling schema");
    }

    // A small protocol the schema loader's downcast tests read both as generated code and as a
    // request of its own, so a test can load it into a fresh loader cheaply.
    capnpc::CompilerCommand::new()
        .file("test-schema-loader-protocol.capnp")
        .embed_schema(true)
        .raw_code_generator_request_path(format!("{out_dir}/schema-loader-protocol.cgr"))
        .run()
        .expect("compiling schema");

    // A schema only the runtime schema loader reads (see the file): dump its request without
    // generating code.
    let output = std::process::Command::new("capnp")
        .args(["compile", "-o-", "test-schema-loader.capnp"])
        .output()
        .expect("running capnp compile");
    assert!(
        output.status.success(),
        "capnp compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(format!("{out_dir}/schema-loader.cgr"), output.stdout)
        .expect("writing schema-loader.cgr");

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
