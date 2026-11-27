use serde_json::{Value, json};

use super::*;
use crate::test_fixtures::TestResult;

fn request(extra: Value) -> Result<RunCommandRequest, serde_json::Error> {
    let mut body = json!({"lease_id": "lease-1", "command": ["true"]});
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    serde_json::from_value(body)
}

#[test]
fn a_plain_command_has_no_violation() -> TestResult {
    assert_eq!(RunCommandViolationError::of(&request(json!({}))?), None);
    assert_eq!(RunCommandViolationError::of(&request(json!({"detached": true}))?), None);
    Ok(())
}

#[test]
fn each_unsupported_option_is_named() -> TestResult {
    let cases = [
        (
            json!({"detached": true, "deadline": "lease"}),
            RunCommandViolationError::LeaseDeadline,
        ),
        (
            json!({"detached": true, "process": {}}),
            RunCommandViolationError::Process,
        ),
    ];
    for (extra, expected) in cases {
        assert_eq!(RunCommandViolationError::of(&request(extra)?), Some(expected));
    }
    Ok(())
}
