#![cfg(not(windows))]

use std::process::Command;

#[test]
fn the_binary_reports_that_it_is_windows_only() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-fence-helper")).output()?;

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)?.contains("Windows only"));
    Ok(())
}
