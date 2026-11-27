use async_nats::Message;
use bytes::Bytes;
use serde_json::json;
use trogon_sandbox_contract::{
    CancelIntent, CapabilitySupport, CommandDeadline, ExecutionId, LeaseError, LeaseId, ProviderKind,
    RunCommandRequest, SnapshotExportError,
};
use trogon_sandbox_protocol::subjects;

use super::super::{
    accept, backend, cancel_intent, checkpoint_running_denial, chunk_subject, extract_cancel_lease_id,
    network_policy_denial, run_command_spec_violation, snapshot_denial,
};
use super::support::TestResult;
use crate::agent::AgentError;

fn run_command(extra: serde_json::Value) -> Result<RunCommandRequest, serde_json::Error> {
    let mut value = json!({"lease_id": "l1", "command": ["cat"], "timeout_seconds": 5});
    if let (Some(base), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    serde_json::from_value(value)
}

fn message(reply: Option<&str>, payload: &'static [u8]) -> Message {
    Message {
        subject: "sandbox.provider.substrate.test".into(),
        reply: reply.map(Into::into),
        payload: Bytes::from_static(payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    }
}

#[test]
fn detached_interactive_stdin_is_accepted() -> TestResult {
    let request = run_command(json!({"detached": true, "stdin": "interactive"}))?;
    assert_eq!(run_command_spec_violation(&request), None);
    Ok(())
}

#[test]
fn a_command_without_special_needs_is_accepted() -> TestResult {
    assert_eq!(run_command_spec_violation(&run_command(json!({}))?), None);
    Ok(())
}

#[test]
fn interactive_stdin_requires_a_detached_command() {
    assert!(run_command(json!({"stdin": "interactive"})).is_err());
}

#[test]
fn lease_deadline_is_still_refused() -> TestResult {
    let request = run_command(json!({"detached": true, "deadline": "lease"}))?;
    assert!(run_command_spec_violation(&request).is_some_and(|reason| reason.contains("lease deadline")));
    assert_eq!(CommandDeadline::default(), CommandDeadline::Timeout);
    Ok(())
}

#[test]
fn launch_cancellation_is_refused() -> TestResult {
    let request = run_command(json!({"require_launch_cancellation": true, "launch_key": "k"}))?;
    assert!(run_command_spec_violation(&request).is_some_and(|reason| reason.contains("launch cancellation")));
    Ok(())
}

#[test]
fn the_profile_advertises_interactive_stdin() {
    assert_eq!(
        crate::profile::PROFILE.isolation.supports_interactive_stdin,
        CapabilitySupport::Supported
    );
}

#[test]
fn a_chunk_lands_on_the_lease_evidence_stream() {
    assert_eq!(
        chunk_subject(&LeaseId::new("01ARZ"), &ExecutionId::new("exec-1")),
        "sandbox.evidence.01ARZ.command.exec-1"
    );
}

#[test]
fn a_cancel_subject_yields_its_lease_id() {
    assert_eq!(
        extract_cancel_lease_id("sandbox.provider.substrate.cancel.01ARZ"),
        Some(LeaseId::new("01ARZ"))
    );
    assert_eq!(extract_cancel_lease_id("sandbox.provider.substrate.cancel."), None);
}

#[test]
fn the_cancel_subject_matches_what_the_broker_publishes() {
    let published = subjects::provider_cancel(ProviderKind::Substrate, &LeaseId::new("01ARZ"));
    assert_eq!(extract_cancel_lease_id(&published), Some(LeaseId::new("01ARZ")));
}

#[test]
fn a_cancel_body_names_its_intent_and_defaults_to_release() -> TestResult {
    assert_eq!(
        cancel_intent(&serde_json::to_vec(&CancelIntent::Purge)?),
        CancelIntent::Purge
    );
    assert_eq!(cancel_intent(b""), CancelIntent::Release);
    assert_eq!(cancel_intent(b"not json"), CancelIntent::Release);
    Ok(())
}

#[test]
fn refusals_name_the_feature_rather_than_looking_like_a_fault() {
    assert!(matches!(
        checkpoint_running_denial(),
        SnapshotExportError::Unsupported { .. }
    ));
    assert!(matches!(snapshot_denial(), SnapshotExportError::Unsupported { .. }));
    assert!(matches!(
        network_policy_denial(),
        LeaseError::InvalidNetworkPolicy { .. }
    ));
}

#[test]
fn an_agent_failure_becomes_a_backend_denial() {
    let denial = backend(&AgentError::NotReady {
        target: "actor".into(),
        seconds: 1,
    });
    assert!(matches!(denial, LeaseError::Backend { .. }));
}

#[test]
fn a_request_without_a_reply_subject_is_discarded() {
    assert!(accept::<serde_json::Value>(&message(None, b"{}"), "test").is_none());
}

#[test]
fn a_request_that_does_not_decode_is_discarded() {
    assert!(accept::<serde_json::Value>(&message(Some("inbox"), b"nope"), "test").is_none());
}

#[test]
fn a_decodable_request_yields_its_reply_subject() {
    let accepted = accept::<serde_json::Value>(&message(Some("inbox"), b"{\"a\":1}"), "test");
    assert!(accepted.is_some_and(|(reply, request)| reply.as_str() == "inbox" && request["a"] == 1));
}

#[test]
fn a_request_that_could_not_be_answered_is_logged_not_raised() -> TestResult {
    super::super::report_failure(super::super::Verb::Lease, Ok(()));
    let error = serde_json::from_str::<u8>("x").err().ok_or("parsed")?;
    super::super::report_failure(
        super::super::Verb::Lease,
        Err(super::super::HandlerError::Encode(error)),
    );
    Ok(())
}
