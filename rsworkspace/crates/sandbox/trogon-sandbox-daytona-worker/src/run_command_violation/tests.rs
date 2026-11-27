use serde_json::{Value, json};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn violation(change: impl FnOnce(&mut Value)) -> Result<Option<&'static str>, serde_json::Error> {
    let mut wire = json!({"lease_id": "lease-1", "command": ["true"]});
    change(&mut wire);
    let request: RunCommandRequest = serde_json::from_value(wire)?;
    Ok(run_command_violation(&request))
}

#[test]
fn a_plain_command_is_accepted() -> TestResult {
    assert_eq!(violation(|_| {})?, None);
    assert_eq!(violation(|wire| wire["run_as_user"] = json!("root"))?, None);
    assert_eq!(violation(|wire| wire["detached"] = json!(true))?, None);
    Ok(())
}

#[test]
fn launch_cancellation_is_refused() -> TestResult {
    let reason = violation(|wire| wire["require_launch_cancellation"] = json!(true))?;
    assert_eq!(reason, Some("this worker cannot make a command launch cancelable"));
    Ok(())
}

#[test]
fn an_empty_command_is_refused() -> TestResult {
    assert_eq!(
        violation(|wire| wire["command"] = json!([]))?,
        Some("command must not be empty")
    );
    Ok(())
}

#[test]
fn streamed_output_is_refused() -> TestResult {
    let reason = violation(|wire| wire["stream_output"] = json!(true))?;
    assert_eq!(reason, Some("this worker cannot stream command output"));
    Ok(())
}

#[test]
fn a_detached_command_cannot_keep_stdin_open_or_name_a_process() -> TestResult {
    let stdin = violation(|wire| {
        wire["detached"] = json!(true);
        wire["stdin"] = json!("interactive");
    })?;
    assert_eq!(stdin, Some("this worker cannot keep a command's stdin open"));
    let process = violation(|wire| {
        wire["detached"] = json!(true);
        wire["process"] = json!({});
    })?;
    assert_eq!(process, Some("named processes and readiness probes are not supported"));
    Ok(())
}

#[test]
fn any_named_user_is_accepted() -> TestResult {
    assert_eq!(violation(|wire| wire["run_as_user"] = json!("someone"))?, None);
    Ok(())
}

#[test]
fn environment_names_must_be_shell_names() -> TestResult {
    assert_eq!(violation(|wire| wire["env"] = json!({"_OK1": "x", "Path": "y"}))?, None);
    for name in ["", "1A", "A-B", "A B", "A=B"] {
        let reason = violation(|wire| wire["env"] = json!({ name: "x" }))?;
        assert_eq!(
            reason,
            Some("environment names are letters, digits and underscores, and do not start with a digit"),
            "{name:?}"
        );
    }
    Ok(())
}
