fn main() {
    let proto_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    println!("cargo:rerun-if-changed={}", proto_root.display());

    let session_v1 = proto_root.join("trogonai/session/v1");
    buffa_build::Config::new()
        .extern_path(
            ".google.protobuf.Timestamp",
            "::buffa_types::google::protobuf::Timestamp",
        )
        .files(&[
            session_v1.join("common.proto"),
            session_v1.join("migration.proto"),
            session_v1.join("artifact_metadata.proto"),
            session_v1.join("context_twin.proto"),
            session_v1.join("capability_schema.proto"),
            session_v1.join("runner_binding.proto"),
            session_v1.join("session_event.proto"),
            session_v1.join("session_snapshot.proto"),
            session_v1.join("prompt_projection.proto"),
        ])
        .includes(&[proto_root])
        .include_file("_include.rs")
        .compile()
        .expect("buffa codegen failed for session contracts");
}
