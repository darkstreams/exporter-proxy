fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/schema/metric_data.capnp");
    capnpc::CompilerCommand::new()
        .output_path(".")
        .file("src/schema/metric_data.capnp")
        .run()
        .unwrap();
}
