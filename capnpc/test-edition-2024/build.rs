fn main() {
    capnpc::CompilerCommand::new()
        .crate_provides("external_crate", [0xe6f94f52f7be8fe2])
        .file("../test/test.capnp")
        .src_prefix("../test/")
        .run()
        .expect("compiling schema");

    // Newtype codegen (traits, erased carriers, use-site tables) compiled under edition 2024.
    capnpc::CompilerCommand::new()
        .file("../test/test-newtype.capnp")
        .file("../test/test-newtype-import.capnp")
        .src_prefix("../test/")
        .run()
        .expect("compiling newtype schema");
}
