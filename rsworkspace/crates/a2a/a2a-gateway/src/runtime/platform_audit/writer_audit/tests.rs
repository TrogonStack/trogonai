use std::time::Duration;

use a2a_nats::agent_id::A2aAgentId;
use trogon_audit::{MemorySink, ReadOnly, SinkError, WriterConfig, WriterId};
use trogon_identity_types::SubjectPrincipal;
use trogon_std::log_capture::{CapturedEvent, CapturedEvents, LevelFilter};

use super::super::decision_event::AllowedLayers;
use super::super::deny_reason::DenyReason;
use super::*;

fn audit() -> (WriterAudit<MemorySink>, MemorySink) {
    let sink = MemorySink::new();
    let writer = AuditWriter::new(
        sink.clone(),
        WriterId::new("a2a-gateway:test").expect("writer"),
        WriterConfig::default(),
    );
    let audit = WriterAudit::new(
        writer,
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    );
    (audit, sink)
}

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").expect("principal")
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("planner").expect("agent")
}

fn event<'a>(
    caller: &'a SubjectPrincipal,
    agent_id: &'a A2aAgentId,
    method: &'a str,
    read_only: ReadOnly,
    verdict: Verdict<'a>,
) -> DecisionEvent<'a> {
    DecisionEvent {
        caller,
        agent_id,
        method,
        read_only,
        trace_id: "0190a7b2-3c4d-7e5f-8a6b-7c8d9e0f1a2b",
        request_id: Some("req-1"),
        request_bytes: 10,
        latency: Duration::from_millis(1),
        rules_fired: &[],
        verdict,
    }
}

fn allow() -> Verdict<'static> {
    Verdict::Allowed(AllowedLayers::default())
}

/// Records `event` and returns the outcome with the one line it logged.
async fn recorded(
    audit: &WriterAudit<MemorySink>,
    method: &str,
    read_only: ReadOnly,
    verdict: Verdict<'_>,
) -> (Result<(), AuditError>, CapturedEvent) {
    let caller = caller();
    let agent = agent();
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::INFO);
    let outcome = audit.record(&event(&caller, &agent, method, read_only, verdict)).await;
    let mut lines: Vec<CapturedEvent> = events
        .events()
        .into_iter()
        .filter(|line| line.message() == Some("gateway decision"))
        .collect();
    assert_eq!(lines.len(), 1, "one line per decision: {lines:?}");
    (outcome, lines.remove(0))
}

#[tokio::test]
async fn an_allow_that_acts_is_stored_before_it_returns_and_logged() {
    let (audit, sink) = audit();
    let (outcome, line) = recorded(&audit, "message/send", ReadOnly::Undeclared, allow()).await;
    assert_eq!(outcome, Ok(()));
    let stored = sink.stored();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].subject(), "platform.v1.audit.decision.payments.allow");
    assert_eq!(line.field("agent"), Some("planner"));
    assert_eq!(line.field("method"), Some("message/send"));
    assert_eq!(line.field("decision"), Some("allow"));
    assert_eq!(line.field("audit"), Some("stored"));
    assert_eq!(line.field("audit_failure"), Some("-"));
}

#[tokio::test]
async fn an_allow_that_acts_and_cannot_be_stored_fails() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, line) = recorded(&audit, "message/send", ReadOnly::Undeclared, allow()).await;
    assert!(matches!(outcome, Err(AuditError::Unavailable(_))), "{outcome:?}");
    assert!(sink.stored().is_empty());
    assert_eq!(line.field("audit"), Some("unavailable"));
    assert_eq!(line.field("audit_failure"), Some("unreachable"));
}

#[tokio::test]
async fn a_deny_stands_when_the_stream_is_down_and_is_stored_once_it_returns() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, line) = recorded(
        &audit,
        "message/send",
        ReadOnly::Undeclared,
        Verdict::Denied(DenyReason::Tier2),
    )
    .await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(line.field("decision"), Some("tier2_denied"));
    assert_eq!(line.field("audit"), Some("retrying"));
    assert!(sink.stored().is_empty());

    sink.recover();
    audit.writer().drain_once().await;
    let stored = sink.stored();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].subject(), "platform.v1.audit.decision.payments.deny");
}

#[tokio::test]
async fn a_read_only_allow_never_fails_even_when_the_stream_is_down() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, line) = recorded(&audit, "tasks/get", ReadOnly::Declared, allow()).await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(line.field("audit"), Some("queued"));
}

#[tokio::test]
async fn a_read_only_allow_is_stored_by_the_drain_once_the_stream_returns() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, _) = recorded(&audit, "tasks/get", ReadOnly::Declared, allow()).await;
    assert_eq!(outcome, Ok(()));
    sink.recover();
    audit.writer().drain_once().await;
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn run_flushes_what_was_buffered_when_the_gateway_stops() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, _) = recorded(
        &audit,
        "message/send",
        ReadOnly::Undeclared,
        Verdict::Denied(DenyReason::Tier1Spicedb),
    )
    .await;
    assert_eq!(outcome, Ok(()));
    sink.recover();

    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
    assert_eq!(sink.stored().len(), 1);
}

#[test]
fn a_record_that_cannot_be_built_is_logged_as_unrecordable() {
    let caller = caller();
    let agent = agent();
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::INFO);
    let name = AuditName::new("has space").expect_err("invalid");
    log(
        &event(&caller, &agent, "message/send", ReadOnly::Undeclared, allow()),
        &Err(AuditError::Name(name)),
    );
    let lines = events.events();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].field("audit"), Some("unavailable"));
    assert_eq!(lines[0].field("audit_failure"), Some("unrecordable"));
}

#[test]
fn every_disposition_has_a_name() {
    assert_eq!(disposition_name(Disposition::Stored), "stored");
    assert_eq!(disposition_name(Disposition::Retrying), "retrying");
    assert_eq!(disposition_name(Disposition::Queued), "queued");
    assert_eq!(disposition_name(Disposition::Dropped), "dropped");
}
