fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Clients only. This worker never serves `ateapi`, so generating servers
    // would produce traits nothing implements.
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&["proto/ateapi.proto"], &["proto"])?;

    Ok(())
}
