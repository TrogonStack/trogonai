fn main() {
    let proto_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../proto");
    println!("cargo:rerun-if-changed={}", proto_root.display());

    buffa_build::Config::new()
        .files(&[proto_root.join("trogonai/catalog/v1/catalog.proto")])
        .includes(&[proto_root])
        .include_file("_include.rs")
        .compile()
        .expect("buffa codegen failed for catalog proto");
}
