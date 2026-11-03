use std::path::PathBuf;

use trogon_std::log_capture::{CapturedEvent, CapturedEvents, CapturedLogs, LevelFilter, LogLevel};

use super::*;

fn logged(event: &AuditEvent<'_>) -> CapturedEvent {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::INFO);
    TracingAudit.record(event);
    let mut recorded = events.events();
    assert_eq!(recorded.len(), 1, "one event per decision: {recorded:?}");
    recorded.remove(0)
}

fn source() -> PolicySource {
    PolicySource::StaticFile {
        path: PathBuf::from("policy.toml"),
    }
}

#[test]
fn a_permit_names_agent_destination_address_and_source() {
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Destination = "api.example.com:443".parse().expect("destination");
    let source = source();
    let line = logged(&AuditEvent {
        agent: &agent,
        destination: Some(&destination),
        resolved_address: Some("93.184.216.34".parse().expect("ip")),
        decision: Decision::Permitted,
        snapshot_source: &source,
    });
    assert_eq!(line.message(), Some("egress decision"));
    assert_eq!(line.field("agent"), Some("agent-1"));
    assert_eq!(line.field("destination"), Some("api.example.com:443"));
    assert_eq!(line.field("resolved_address"), Some("93.184.216.34"));
    assert_eq!(line.field("decision"), Some("permitted"));
    assert_eq!(line.field("fault"), Some("-"));
    assert_eq!(line.field("snapshot_source"), Some("static_file:policy.toml"));
}

#[test]
fn a_refusal_carries_the_fault_code_and_no_address() {
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Destination = "evil.example:443".parse().expect("destination");
    let source = source();
    let line = logged(&AuditEvent {
        agent: &agent,
        destination: Some(&destination),
        resolved_address: None,
        decision: Decision::Refused(Fault::NoPermit),
        snapshot_source: &source,
    });
    assert_eq!(line.field("decision"), Some("refused"));
    assert_eq!(line.field("fault"), Some("NO_PERMIT"));
    assert_eq!(line.field("resolved_address"), Some("-"));
}

#[test]
fn a_request_that_never_named_a_destination_still_gets_an_event() {
    let agent = AgentId::new("agent-1").expect("agent");
    let source = source();
    let line = logged(&AuditEvent {
        agent: &agent,
        destination: None,
        resolved_address: None,
        decision: Decision::Refused(Fault::BadRequest),
        snapshot_source: &source,
    });
    assert_eq!(line.field("destination"), Some("-"));
    assert_eq!(line.field("fault"), Some("BAD_REQUEST"));
}

#[test]
fn the_decision_reaches_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let agent = AgentId::new("agent-1").expect("agent");
    let destination: Destination = "api.example.com:443".parse().expect("destination");
    let source = source();
    TracingAudit.record(&AuditEvent {
        agent: &agent,
        destination: Some(&destination),
        resolved_address: Some("93.184.216.34".parse().expect("ip")),
        decision: Decision::Permitted,
        snapshot_source: &source,
    });
    let records = logs.records();
    let record = records
        .iter()
        .find(|record| record.target == "egress_audit")
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

#[test]
fn decision_exposes_its_fault() {
    assert_eq!(Decision::Permitted.fault(), None);
    assert_eq!(Decision::Refused(Fault::SniMismatch).fault(), Some(Fault::SniMismatch));
}
