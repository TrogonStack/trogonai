use std::error::Error;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    FileOpResponse, LeaseError, PollCommandResponse, ReadFileResponse, RestartCommandResponse, RunCommandResponse,
};

use super::*;

type Outcome = Result<(), Box<dyn Error>>;

fn denied() -> LeaseError {
    LeaseError::Backend {
        message: "no".to_owned(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_value(value)?)
}

fn completed_json(exit_code: i32) -> Value {
    json!({ "outcome": "completed", "exit_code": exit_code, "stdout": [107], "stderr": [] })
}

#[test]
fn a_grant_is_read_from_a_reply_that_describes_one() -> Outcome {
    let reply = json!({ "sandbox_id": "s", "evidence_subject": "e" });
    assert_eq!(granted(reply, "a grant")?.evidence_subject, "e");
    Ok(())
}

#[test]
fn a_reply_that_is_not_a_grant_is_a_mismatch() {
    assert!(granted(json!({ "error": "x" }), "a grant").is_err());
}

#[test]
fn a_completed_command_yields_its_output() -> Outcome {
    assert_eq!(completed(parse(completed_json(0))?)?.exit_code, 0);
    Ok(())
}

#[test]
fn a_command_that_did_not_complete_is_a_mismatch() {
    assert!(completed(RunCommandResponse::Denied(denied())).is_err());
}

#[test]
fn a_started_command_yields_its_execution_id() -> Outcome {
    let response = parse(json!({ "outcome": "started", "execution_id": "x1" }))?;
    assert_eq!(started(response)?.as_str(), "x1");
    Ok(())
}

#[test]
fn a_command_that_did_not_start_is_a_mismatch() {
    assert!(started(RunCommandResponse::Denied(denied())).is_err());
}

#[test]
fn a_restarted_command_yields_its_new_execution_id() -> Outcome {
    let response = parse(json!({ "outcome": "restarted", "execution_id": "x2" }))?;
    assert_eq!(restarted(response)?.as_str(), "x2");
    Ok(())
}

#[test]
fn a_command_that_did_not_restart_is_a_mismatch() {
    assert!(restarted(RestartCommandResponse::NotFound).is_err());
}

#[test]
fn a_poll_tells_running_from_completed() -> Outcome {
    assert!(finished(PollCommandResponse::Running)?.is_none());
    assert!(finished(parse(completed_json(0))?)?.is_some());
    Ok(())
}

#[test]
fn a_poll_that_is_neither_running_nor_completed_is_a_mismatch() {
    assert!(finished(PollCommandResponse::NotFound).is_err());
}

#[test]
fn file_content_yields_its_bytes() -> Outcome {
    let response = parse(json!({
        "outcome": "content", "data": "aGk=", "offset": 0, "total_size": 2, "eof": true, "truncated": false
    }))?;
    assert!(content(response, "/p").is_ok());
    Ok(())
}

#[test]
fn a_reply_without_content_is_a_mismatch() {
    assert!(content(FileOpResponse::Renamed, "/p").is_err());
}

#[test]
fn eval_state_is_read_from_a_successful_run_or_a_refusal() -> Outcome {
    assert_eq!(state_from_eval(parse(completed_json(0))?)?, Some(b"k".to_vec()));
    assert_eq!(state_from_eval(RunCommandResponse::Denied(denied()))?, None);
    Ok(())
}

#[test]
fn eval_state_from_a_failed_run_is_a_mismatch() -> Outcome {
    assert!(state_from_eval(parse(completed_json(1))?).is_err());
    Ok(())
}

#[test]
fn file_state_is_read_from_content_or_an_absence() -> Outcome {
    assert_eq!(
        state_from_file(ReadFileResponse::Content { data: vec![1] })?,
        Some(vec![1])
    );
    assert_eq!(state_from_file(ReadFileResponse::NotFound)?, None);
    assert_eq!(state_from_file(ReadFileResponse::Denied(denied()))?, None);
    Ok(())
}

#[test]
fn an_archive_with_entries_yields_its_data() -> Outcome {
    let response = parse(json!({ "outcome": "archive", "data": "aGk=", "entries": 1, "bytes": 2 }))?;
    assert!(archive(response).is_ok());
    Ok(())
}

#[test]
fn an_empty_archive_is_a_mismatch() -> Outcome {
    let response = parse(json!({ "outcome": "archive", "data": "", "entries": 0, "bytes": 0 }))?;
    assert!(archive(response).is_err());
    Ok(())
}

#[test]
fn an_extraction_counts_only_when_it_unpacked_entries() -> Outcome {
    let some = parse(json!({ "outcome": "extracted", "entries": 1, "bytes": 1 }))?;
    let none = parse(json!({ "outcome": "extracted", "entries": 0, "bytes": 0 }))?;
    assert!(extracted(&some));
    assert!(!extracted(&none));
    assert!(!extracted(&FileOpResponse::Renamed));
    Ok(())
}
