use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sandbox(session_id: &str) -> LeaseSandbox {
    LeaseSandbox {
        name: "sbx-a".to_owned(),
        session_id: session_id.to_owned(),
        routes: BTreeMap::new(),
    }
}

fn request(change: impl FnOnce(&mut Value)) -> Result<RunCommandRequest, serde_json::Error> {
    let mut wire = json!({"lease_id": "lease-1", "command": ["echo", "hi"], "timeout_seconds": 30});
    change(&mut wire);
    serde_json::from_value(wire)
}

fn execution(request: &RunCommandRequest) -> Execution {
    Execution::new(&sandbox("sess_1"), request, 4096)
}

fn logs(command_id: Option<&str>, exit: Option<i32>) -> Collected {
    let mut logs = Collected::with_limit(1 << 20);
    let frame = json!({"stream": "stdout", "data": "0\n0\n\n\n0\n\n\n"});
    logs.line(frame.to_string().as_bytes());
    logs.command_id = command_id.map(str::to_owned);
    logs.exit = exit;
    logs
}

#[test]
fn a_session_path_is_built_from_a_safe_session_id() -> TestResult {
    assert_eq!(
        VercelExec::session_path(&sandbox("sess_1"), "/cmd")?,
        "/v2/sandboxes/sessions/sess_1/cmd"
    );
    assert!(matches!(
        VercelExec::session_path(&sandbox("../x"), ""),
        Err(VercelApiError::Rejected { status: 400, .. })
    ));
    Ok(())
}

#[test]
fn refusals_carry_their_reason() {
    assert!(matches!(
        denied(&"gone"),
        LeaseError::Backend { ref message } if message == "gone"
    ));
    assert!(matches!(
        invalid_spec("bad"),
        LeaseError::InvalidSpec { ref reason } if reason == "bad"
    ));
}

#[test]
fn an_attached_body_waits_and_is_bound_by_the_timeout() -> TestResult {
    let body = VercelExec::captured_command_body(&request(|_| {})?, true, "cap1");
    assert_eq!(body["command"], "sh");
    assert_eq!(body["sudo"], false);
    assert_eq!(body["wait"], true);
    assert_eq!(body["timeout"], 30_000);
    assert!(body.get("cwd").is_none());
    let args = body["args"].as_array().ok_or("args")?;
    assert_eq!(args[0], "-c");
    assert!(args[1].as_str().ok_or("script")?.starts_with("set -- cap1 \"$@\"; "));
    assert_eq!(args[3], "30s");
    assert_eq!(args[4], DEFAULT_COMMAND_CWD);
    assert_eq!(args[5], "echo");
    assert_eq!(args[6], "hi");
    Ok(())
}

#[test]
fn cwd_env_and_user_reach_the_body() -> TestResult {
    let body = VercelExec::captured_command_body(
        &request(|wire| {
            wire["cwd"] = json!("/work");
            wire["env"] = json!({"A": "b"});
            wire["run_as_user"] = json!("root");
        })?,
        false,
        "cap1",
    );
    assert_eq!(body["sudo"], true);
    assert_eq!(body["cwd"], "/work");
    assert!(body.get("wait").is_none());
    let args = body["args"].as_array().ok_or("args")?;
    assert_eq!(args[4], "/work");
    assert_eq!(args[5], "A=b");
    assert_eq!(args[6], "echo");
    Ok(())
}

#[test]
fn a_lease_bound_command_has_no_deadline_of_its_own() -> TestResult {
    let body = VercelExec::captured_command_body(
        &request(|wire| {
            wire["detached"] = json!(true);
            wire["deadline"] = json!("lease");
            wire["timeout_seconds"] = json!(0);
        })?,
        false,
        "cap1",
    );
    assert!(body.get("timeout").is_none());
    let args = body["args"].as_array().ok_or("args")?;
    assert!(args[1].as_str().ok_or("script")?.contains("capture_without_deadline()"));
    assert_eq!(args[3], "1s");
    Ok(())
}

#[test]
fn a_receipt_turns_its_backing_logs_into_output() -> TestResult {
    let request = request(|_| {})?;
    let output = VercelExec::receipt_output(logs(Some("cmd_1"), Some(0)), &execution(&request), "cmd_1")?;
    assert_eq!(output.exit_code, 0);
    let unnamed = VercelExec::receipt_output(logs(None, None), &execution(&request), "cmd_1")?;
    assert_eq!(unnamed.exit_code, 0);
    Ok(())
}

#[test]
fn a_receipt_naming_another_command_or_a_failed_one_is_refused() -> TestResult {
    let request = request(|_| {})?;
    let execution = execution(&request);
    assert!(matches!(
        VercelExec::receipt_output(logs(Some("cmd_2"), Some(0)), &execution, "cmd_1"),
        Err(VercelExecError::Capture(_))
    ));
    assert!(matches!(
        VercelExec::receipt_output(logs(Some("cmd_1"), Some(2)), &execution, "cmd_1"),
        Err(VercelExecError::Capture(_))
    ));
    assert!(matches!(
        VercelExec::receipt_output(logs(None, None), &execution, "a/b"),
        Err(VercelExecError::Api(VercelApiError::Rejected { status: 400, .. }))
    ));
    Ok(())
}
