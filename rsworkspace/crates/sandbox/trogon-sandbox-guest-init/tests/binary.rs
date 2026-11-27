use std::process::Command;

#[test]
fn refuses_to_run_outside_a_guest() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-guest-init")).output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
    Ok(())
}
