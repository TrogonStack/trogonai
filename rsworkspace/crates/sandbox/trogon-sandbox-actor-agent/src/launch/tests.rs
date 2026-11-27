use std::error::Error;

use serde_json::json;
use trogon_sandbox_contract::RunCommandRequest;

use super::*;

fn request(overrides: serde_json::Value) -> Result<RunCommandRequest, Box<dyn Error>> {
    let mut base = json!({"lease_id": "lease", "command": ["echo", "hi"], "timeout_seconds": 5});
    if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
        base.extend(overrides.clone());
    }
    Ok(serde_json::from_value(base)?)
}

fn reason(request: &RunCommandRequest) -> Result<String, Box<dyn Error>> {
    match Launch::parse(request) {
        Err(LeaseError::InvalidSpec { reason }) => Ok(reason),
        other => Err(format!("expected a refusal, got {other:?}").into()),
    }
}

#[test]
fn an_ordinary_command_is_split_into_program_and_arguments() -> Result<(), Box<dyn Error>> {
    let request = request(json!({}))?;
    let launch = Launch::parse(&request)?;
    assert_eq!(launch.program, "echo");
    assert_eq!(launch.args, ["hi"]);
    Ok(())
}

#[test]
fn each_unsupported_shape_is_refused_with_its_own_reason() -> Result<(), Box<dyn Error>> {
    assert!(reason(&request(json!({"require_launch_cancellation": true}))?)?.contains("launch cancellation"));
    assert!(reason(&request(json!({"command": []}))?)?.contains("must not be empty"));
    assert!(reason(&request(json!({"timeout_seconds": 0}))?)?.contains("timeout_seconds"));
    assert!(reason(&request(json!({"detached": true, "deadline": "lease"}))?)?.contains("lease deadline"));
    Ok(())
}
