use super::*;

#[test]
fn recorded_frames_appear_in_the_registry_output() -> Result<(), Box<dyn std::error::Error>> {
    let registry = Registry::new();
    register(&registry)?;
    record(0);
    record(3);
    record(4);
    let text = prometheus::TextEncoder::new().encode_to_string(&registry.gather())?;
    assert!(text.contains("sandbox_firecracker_spoofed_frames_total 7"), "{text}");
    Ok(())
}
