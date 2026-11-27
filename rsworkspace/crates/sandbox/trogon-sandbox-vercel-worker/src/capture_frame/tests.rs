use std::collections::BTreeMap;

use serde_json::{Value, json};
use trogon_sandbox_contract::{CommandTimeoutStatus, RunCommandRequest};

use super::*;
use crate::LeaseSandbox;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const STREAM_LIMIT: u64 = GUEST_STREAM_LIMIT as u64;

fn execution(change: impl FnOnce(&mut Value)) -> Result<Execution, serde_json::Error> {
    let mut wire = json!({"lease_id": "lease-1", "command": ["true"], "timeout_seconds": 30});
    change(&mut wire);
    let request: RunCommandRequest = serde_json::from_value(wire)?;
    let sandbox = LeaseSandbox {
        name: "sbx-a".to_owned(),
        session_id: "sess_1".to_owned(),
        routes: BTreeMap::new(),
    };
    Ok(Execution::new(&sandbox, &request, 4096))
}

fn collected_from(stdout: &str) -> Collected {
    let mut collected = Collected::with_limit(1 << 20);
    collected.line(json!({"stream": "stdout", "data": stdout}).to_string().as_bytes());
    collected
}

fn output(frame: &str, execution: &Execution) -> Result<CommandOutput, VercelExecError> {
    captured_output(collected_from(frame), execution, 0)
}

fn is_capture_error(result: Result<CommandOutput, VercelExecError>) -> bool {
    matches!(result, Err(VercelExecError::Capture(_)))
}

#[test]
fn a_complete_frame_becomes_the_command_output() -> TestResult {
    let result = output("3\n2\naGk=\n\n0\n\n\n", &execution(|_| {})?)?;
    assert_eq!(result.exit_code, 3);
    assert_eq!(result.stdout, b"hi");
    assert!(result.stderr.is_empty());
    assert_eq!(result.timed_out, CommandTimeoutStatus::WithinDeadline);
    Ok(())
}

#[test]
fn a_timed_out_frame_is_reported_unless_the_command_is_lease_bound() -> TestResult {
    let frame = "timeout\n0\n\n\n0\n\n\n";
    let result = output(frame, &execution(|_| {})?)?;
    assert_eq!(result.exit_code, -1);
    assert_eq!(result.timed_out, CommandTimeoutStatus::TimedOut);
    let lease_bound = execution(|wire| {
        wire["detached"] = json!(true);
        wire["deadline"] = json!("lease");
    })?;
    assert!(is_capture_error(output(frame, &lease_bound)));
    Ok(())
}

#[test]
fn a_failed_carrier_or_incomplete_stream_is_refused() -> TestResult {
    let execution = execution(|_| {})?;
    let frame = "0\n0\n\n\n0\n\n\n";
    assert!(is_capture_error(captured_output(collected_from(frame), &execution, 1)));
    let mut truncated = collected_from(frame);
    truncated.truncated = true;
    assert!(is_capture_error(captured_output(truncated, &execution, 0)));
    let mut errored = collected_from(frame);
    errored.error = Some("boom".to_owned());
    assert!(is_capture_error(captured_output(errored, &execution, 0)));
    let mut noisy = collected_from(frame);
    noisy.line(br#"{"stream":"stderr","data":"x"}"#);
    assert!(is_capture_error(captured_output(noisy, &execution, 0)));
    Ok(())
}

#[test]
fn a_frame_that_is_not_ascii_or_not_eight_fields_is_refused() -> TestResult {
    let execution = execution(|_| {})?;
    for frame in ["0\n0\n\n\n0\n\n", "0\n0\n\n\n0\n\n\nextra", "caf\u{e9}\n0\n\n\n0\n\n\n"] {
        assert!(is_capture_error(output(frame, &execution)), "{frame:?}");
    }
    let mut not_utf8 = Collected::with_limit(64);
    not_utf8.stdout.push(&[0xff, 0xfe]);
    assert!(is_capture_error(captured_output(not_utf8, &execution, 0)));
    Ok(())
}

#[test]
fn stream_sizes_and_bytes_must_agree() -> TestResult {
    let execution = execution(|_| {})?;
    let past_limit = STREAM_LIMIT + 1;
    for frame in [
        "0\nmany\n\n\n0\n\n\n".to_owned(),
        "0\n2\naGk\n\n0\n\n\n".to_owned(),
        "0\n2\n====\n\n0\n\n\n".to_owned(),
        "0\n2\naG!=\n\n0\n\n\n".to_owned(),
        "0\n5\naGk=\n\n0\n\n\n".to_owned(),
        format!("0\n{past_limit}\naGk=\n\n0\n\n\n"),
        "0\n0\n\n\n18446744073709551615\n\n\n".to_owned(),
    ] {
        assert!(is_capture_error(output(&frame, &execution)), "{frame:?}");
    }
    Ok(())
}

#[test]
fn a_frame_the_protocol_rejects_is_malformed() -> TestResult {
    let execution = execution(|_| {})?;
    assert!(is_capture_error(output("bad\n0\n\n\n0\n\n\n", &execution)));
    Ok(())
}
