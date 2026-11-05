use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Notify;
use trogon_audit::{MemorySink, ReasonCode, SealedRecord, SinkError, WriterConfig, WriterId};
use trogon_std::log_capture::{CapturedEvent, CapturedEvents, CapturedLogs, LevelFilter, LogLevel};

use super::*;
use crate::agent_id::AgentId;
use crate::destination::Destination;
use crate::policy_source::PolicySource;

fn audit() -> (WriterAudit<MemorySink>, MemorySink) {
    let sink = MemorySink::new();
    let writer = AuditWriter::new(
        sink.clone(),
        WriterId::new("egress-proxy:test").expect("writer"),
        WriterConfig::default(),
    );
    let audit = WriterAudit::new(
        writer,
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    );
    (audit, sink)
}

fn source() -> PolicySource {
    PolicySource::StaticFile {
        path: PathBuf::from("policy.toml"),
    }
}

fn event<'a>(
    agent: &'a AgentId,
    destination: Option<&'a Destination>,
    source: &'a PolicySource,
    decision: Decision,
) -> AuditEvent<'a> {
    AuditEvent {
        agent,
        destination,
        resolved_address: destination.map(|_| "93.184.216.34".parse().expect("ip")),
        decision,
        snapshot_source: source,
    }
}

/// Records `decision` and returns the outcome with the one line it logged.
async fn recorded(
    audit: &WriterAudit<MemorySink>,
    destination: Option<&str>,
    decision: Decision,
) -> (Result<(), AuditError>, CapturedEvent) {
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Option<Destination> = destination.map(|text| text.parse().expect("destination"));
    let source = source();
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::INFO);
    let outcome = audit
        .record(&event(&agent, destination.as_ref(), &source, decision))
        .await;
    let mut lines: Vec<CapturedEvent> = events
        .events()
        .into_iter()
        .filter(|line| line.message() == Some("egress decision"))
        .collect();
    assert_eq!(lines.len(), 1, "one line per decision: {lines:?}");
    (outcome, lines.remove(0))
}

#[tokio::test]
async fn a_permit_is_stored_and_logged_with_its_address() {
    let (audit, sink) = audit();
    let (outcome, line) = recorded(&audit, Some("api.example.com:443"), Decision::Permitted).await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(sink.stored().len(), 1);
    assert_eq!(line.message(), Some("egress decision"));
    assert_eq!(line.field("agent"), Some("agent-1"));
    assert_eq!(line.field("destination"), Some("api.example.com:443"));
    assert_eq!(line.field("resolved_address"), Some("93.184.216.34"));
    assert_eq!(line.field("decision"), Some("permitted"));
    assert_eq!(line.field("fault"), Some("-"));
    assert_eq!(line.field("snapshot_source"), Some("static_file:policy.toml"));
    assert_eq!(line.field("audit"), Some("stored"));
    assert_eq!(line.field("audit_failure"), Some("-"));
}

#[tokio::test]
async fn a_permit_that_cannot_be_stored_fails_and_is_logged_as_the_refusal_it_became() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, line) = recorded(&audit, Some("api.example.com:443"), Decision::Permitted).await;
    assert!(matches!(outcome, Err(AuditError::Unavailable(_))));
    assert!(sink.stored().is_empty());
    assert_eq!(line.field("decision"), Some("refused"));
    assert_eq!(line.field("fault"), Some("AUDIT_UNAVAILABLE"));
    assert_eq!(line.field("audit"), Some("unavailable"));
    assert_eq!(line.field("audit_failure"), Some("unreachable"));
}

#[tokio::test]
async fn a_refusal_stands_when_the_stream_is_down_and_is_stored_once_it_returns() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, line) = recorded(&audit, Some("evil.example:443"), Decision::Refused(Fault::NoPermit)).await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(line.field("decision"), Some("refused"));
    assert_eq!(line.field("fault"), Some("NO_PERMIT"));
    assert_eq!(line.field("audit"), Some("retrying"));
    assert!(sink.stored().is_empty());

    sink.recover();
    audit.writer.drain_once().await;
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn a_request_that_never_named_a_destination_is_still_recorded_and_logged() {
    let (audit, sink) = audit();
    let (outcome, line) = recorded(&audit, None, Decision::Refused(Fault::BadRequest)).await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(sink.stored().len(), 1);
    assert_eq!(line.field("destination"), Some("-"));
    assert_eq!(line.field("resolved_address"), Some("-"));
    assert_eq!(line.field("fault"), Some("BAD_REQUEST"));
}

#[tokio::test]
async fn run_flushes_what_was_buffered_when_the_server_stops() {
    let (audit, sink) = audit();
    sink.fail_all(SinkError::Unreachable);
    let (outcome, _) = recorded(&audit, Some("evil.example:443"), Decision::Refused(Fault::NoPermit)).await;
    assert_eq!(outcome, Ok(()));
    sink.recover();

    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
    assert_eq!(sink.stored().len(), 1);
}

#[test]
fn a_record_that_cannot_be_built_is_logged_as_unrecordable() {
    let agent = AgentId::new("agent-1").expect("agent");
    let source = source();
    let reason = ReasonCode::new("Not Snake").expect_err("invalid");
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::INFO);
    log(
        &event(&agent, None, &source, Decision::Permitted),
        &Err(AuditError::Reason(reason)),
    );
    let name = AuditName::new("has space").expect_err("invalid");
    log(
        &event(&agent, None, &source, Decision::Refused(Fault::NoPermit)),
        &Err(AuditError::Name(name)),
    );
    let lines = events.events();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].field("fault"), Some("AUDIT_UNAVAILABLE"));
    assert_eq!(lines[0].field("audit_failure"), Some("unrecordable"));
    assert_eq!(lines[1].field("fault"), Some("NO_PERMIT"));
    assert_eq!(lines[1].field("audit_failure"), Some("unrecordable"));
}

#[test]
fn every_disposition_has_a_name() {
    assert_eq!(disposition_name(Disposition::Stored), "stored");
    assert_eq!(disposition_name(Disposition::Retrying), "retrying");
    assert_eq!(disposition_name(Disposition::Queued), "queued");
    assert_eq!(disposition_name(Disposition::Dropped), "dropped");
}

#[tokio::test]
async fn the_decision_reaches_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let (audit, _sink) = audit();
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Destination = "api.example.com:443".parse().expect("destination");
    let source = source();
    audit
        .record(&event(&agent, Some(&destination), &source, Decision::Permitted))
        .await
        .expect("stored");
    let records = logs.records();
    let record = records
        .iter()
        .find(|record| record.target == AUDIT_LOG_TARGET)
        .unwrap_or_else(|| panic!("no audit record in {} records", records.len()));
    assert_eq!(record.level, LogLevel::Info);
    for expected in [
        "agent=agent-1",
        "decision=\"permitted\"",
        "destination=\"api.example.com:443\"",
    ] {
        assert!(
            record.message.contains(expected),
            "missing {expected} in {}",
            record.message
        );
    }
}

/// Holds every publish until released, and says when one has started.
#[derive(Clone)]
struct HeldSink {
    inner: MemorySink,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl trogon_audit::AuditSink for HeldSink {
    async fn publish(&self, record: &SealedRecord) -> Result<(), SinkError> {
        self.started.notify_one();
        self.release.notified().await;
        self.inner.publish(record).await
    }
}

#[tokio::test]
async fn a_refusal_whose_connection_is_aborted_mid_write_is_still_stored() {
    let sink = MemorySink::new();
    let held = HeldSink {
        inner: sink.clone(),
        started: Arc::default(),
        release: Arc::default(),
    };
    let writer = AuditWriter::new(
        held.clone(),
        WriterId::new("egress-proxy:test").expect("writer"),
        WriterConfig::default(),
    );
    let audit = Arc::new(WriterAudit::new(
        writer,
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    ));

    let connection = tokio::spawn({
        let audit = Arc::clone(&audit);
        async move {
            let agent = AgentId::new("agent-1").expect("agent");
            let destination: Destination = "evil.example:443".parse().expect("destination");
            let source = source();
            let _ = audit
                .record(&event(
                    &agent,
                    Some(&destination),
                    &source,
                    Decision::Refused(Fault::NoPermit),
                ))
                .await;
        }
    });
    held.started.notified().await;
    connection.abort();
    assert!(connection.await.expect_err("aborted").is_cancelled());

    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let run = tokio::spawn({
        let audit = Arc::clone(&audit);
        async move { audit.run(shutdown).await }
    });
    held.release.notify_one();
    run.await.expect("run");

    assert_eq!(sink.stored().len(), 1);
}
