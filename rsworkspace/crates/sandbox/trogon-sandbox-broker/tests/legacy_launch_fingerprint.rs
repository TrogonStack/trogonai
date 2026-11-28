//! Launch keys are fingerprinted from the serialized request, so a serialization
//! change would make a rolling upgrade reject a retry of an unchanged legacy launch.

use sha2::{Digest as _, Sha256};
use trogon_sandbox_broker::command_launch::fingerprint;
use trogon_sandbox_contract::{LaunchCancellation, RunCommandRequest};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const LEGACY_BYTES: &str = r#"{"lease_id":"legacy-launch","command":["true"],"env":{},"timeout_seconds":60,"detached":false,"stream_output":false,"stdin":"closed","output_retention":"buffer","deadline":"timeout"}"#;
const LEGACY_FINGERPRINT: &str = "d59162d9203a4428a30e18cd04d78039e4db89c99c1d2dc3ecca8c63dc15e1cd";

fn legacy_request() -> Result<RunCommandRequest, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "lease_id": "legacy-launch",
        "command": ["true"],
        "launch_key": "legacy-key",
    }))
}

#[test]
fn a_legacy_run_command_keeps_its_original_wire_shape() -> TestResult {
    let mut request = legacy_request()?;
    request.launch_key = None;
    assert_eq!(serde_json::to_string(&request)?, LEGACY_BYTES);
    Ok(())
}

#[test]
fn a_legacy_run_command_keeps_its_launch_fingerprint() -> TestResult {
    let request = legacy_request()?;
    assert_eq!(hex::encode(Sha256::digest(LEGACY_BYTES)), LEGACY_FINGERPRINT);
    assert_eq!(fingerprint(&request)?, LEGACY_FINGERPRINT);
    Ok(())
}

#[test]
fn opting_into_launch_cancellation_changes_the_fingerprint() -> TestResult {
    let request = legacy_request()?;
    assert!(
        serde_json::to_value(&request)?
            .get("require_launch_cancellation")
            .is_none()
    );

    let mut opted_in = request;
    opted_in.require_launch_cancellation = LaunchCancellation::Required;
    assert_eq!(serde_json::to_value(&opted_in)?["require_launch_cancellation"], true);
    assert_ne!(fingerprint(&opted_in)?, LEGACY_FINGERPRINT);
    Ok(())
}
