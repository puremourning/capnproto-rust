fn main() {
    ::capnpc::CompilerCommand::new()
        .file("test.capnp")
        .file("test-newtype.capnp")
        .run()
        .unwrap();
}
