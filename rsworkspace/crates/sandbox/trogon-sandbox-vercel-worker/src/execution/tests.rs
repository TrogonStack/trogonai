use serde_json::{Value, json};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sandbox() -> LeaseSandbox {
    LeaseSandbox {
        name: "sbx-a".to_owned(),
        session_id: "sess_1".to_owned(),
        routes: std::collections::BTreeMap::new(),
    }
}

fn execution(change: impl FnOnce(&mut Value)) -> Result<Execution, serde_json::Error> {
    let mut wire = json!({"lease_id": "lease-1", "command": ["true"], "timeout_seconds": 30});
    change(&mut wire);
    let request: RunCommandRequest = serde_json::from_value(wire)?;
    Ok(Execution::new(&sandbox(), &request, 4096))
}

#[test]
fn an_attached_command_is_bound_by_its_timeout() -> TestResult {
    let execution = execution(|_| {})?;
    assert_eq!(execution.lease_id, "lease-1");
    assert_eq!(execution.session_id, "sess_1");
    assert_eq!(execution.max_output_bytes, 4096);
    assert_eq!(execution.timeout_seconds, 30);
    assert_eq!(execution.deadline, CommandDeadline::Timeout);
    assert!(!execution.sudo);
    assert_eq!(execution.capture_id.len(), 32);
    Ok(())
}

#[test]
fn a_detached_command_keeps_its_deadline_and_a_user_means_sudo() -> TestResult {
    let execution = execution(|wire| {
        wire["detached"] = json!(true);
        wire["deadline"] = json!("lease");
        wire["run_as_user"] = json!("root");
        wire["timeout_seconds"] = json!(0);
    })?;
    assert_eq!(execution.deadline, CommandDeadline::Lease);
    assert!(execution.sudo);
    assert_eq!(execution.timeout_seconds, 1);
    Ok(())
}

#[test]
fn each_execution_gets_its_own_capture_id_and_state() -> TestResult {
    let first = execution(|_| {})?;
    let second = execution(|_| {})?;
    assert_ne!(first.capture_id, second.capture_id);
    assert!(!Arc::ptr_eq(&first.state, &second.state));
    let state = ExecutionState::default();
    assert!(state.terminal.is_none() && state.completed.is_none() && !state.cleanup_pending);
    Ok(())
}
