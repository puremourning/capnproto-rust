fn main() {
    capnpc::CompilerCommand::new()
        .file("external.capnp")
        .file("external-types.capnp")
        .run()
        .expect("compiling schema");
}
