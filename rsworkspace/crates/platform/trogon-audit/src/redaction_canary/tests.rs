use trogon_std::CapturedEvents;
use trogon_std::log_capture::LevelFilter;

use super::*;
use crate::audit_name::AuditName;
use crate::audit_writer::AuditWriter;
use crate::change_facts::ChangeFacts;
use crate::change_terminal::ChangeTerminal;
use crate::credential_facts::CredentialFacts;
use crate::credential_terminal::CredentialTerminal;
use crate::decision_facts::DecisionFacts;
use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::fixtures::{identity, name, scope, sealed_at, stamper};
use crate::layer_facts::LayerFacts;
use crate::memory_sink::MemorySink;
use crate::reason_code::ReasonCode;
use crate::record::Record;
use crate::sink_error::SinkError;
use crate::surface::Surface;
use crate::writer_config::WriterConfig;

fn leaks(haystack: &[u8]) -> bool {
    haystack
        .windows(marker().len())
        .any(|window| window == marker().as_bytes())
}

#[test]
fn the_detector_sees_a_planted_canary() {
    for canary in canaries() {
        assert!(leaks(format!("prefix {canary} suffix").as_bytes()));
    }
    assert!(!leaks(b"api.example.com"));
}

#[test]
fn no_name_can_hold_a_canary() {
    for canary in canaries() {
        let error = AuditName::new(canary).unwrap_err();
        assert!(!leaks(format!("{error} {error:?}").as_bytes()), "{canary}");
    }
}

#[test]
fn no_reason_code_can_hold_a_canary() {
    for canary in canaries() {
        let error = ReasonCode::new(canary).unwrap_err();
        assert!(!leaks(format!("{error} {error:?}").as_bytes()), "{canary}");
    }
}

/// Everything a surface can put in a record, built from legitimate names while the
/// canaries are alive in the same scope, as they are on a request that carries them.
fn records(
    stamper: &crate::stamper::Stamper<trogon_std::time::FixedEpochClock, crate::fixtures::SequentialIds>,
) -> Vec<Record> {
    let mut layer = LayerFacts::new(name("policy"), DecisionOutcome::Deny);
    layer.reason = Some(ReasonCode::new("no_rule").unwrap());
    layer.rule_ids = vec![name("rule-1")];
    let mut facts = DecisionFacts::new(Surface::Egress, name("api.example.com"), DecisionOutcome::Deny);
    facts.reason = Some(ReasonCode::new("no_rule").unwrap());
    facts.layers = vec![layer];
    let allow = DecisionFacts::new(
        Surface::A2aIngress,
        name("tasks/send"),
        DecisionOutcome::Allow(Enforcement::Enforce),
    );
    let mut credential = CredentialFacts::new(CredentialTerminal::Inject, name("github_token"), name("binding_1"));
    credential.destination = Some(name("api.example.com"));
    let change = ChangeFacts {
        kind: ChangeTerminal::Credential,
        resource_name: name("credentials/api"),
        event_type: name("rotated"),
        source_stream: name("CRED_EVENTS"),
        stream_sequence: 7,
        event_id: name("evt_9"),
    };
    vec![
        Record::decision(scope(), facts, stamper.stamp()),
        Record::decision(scope(), allow, stamper.stamp()),
        Record::credential_use(scope(), credential, stamper.stamp()),
        Record::change(scope(), change, stamper.stamp()),
    ]
}

#[tokio::test]
async fn a_canary_in_scope_never_reaches_a_record_a_debug_rendering_or_a_log_line() {
    let ambient = canaries();
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let stamper = stamper();
    let sink = MemorySink::new();
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper);

    let mut rendered = Vec::new();
    for record in records(&stamper) {
        rendered.push(format!("{record:?}"));
        rendered.push(format!("{:?}", sealed_at(record.clone(), 1)));
        sink.fail_next(1, SinkError::Unreachable);
        let _ = writer.write(record).await;
    }
    sink.recover();
    writer.drain_once().await;
    rendered.push(format!("{writer:?}"));
    rendered.push(format!("{:?}", writer.stats()));
    rendered.push(format!("{:?}", captured.events()));

    let stored = sink.stored();
    assert!(!stored.is_empty());
    assert!(
        stored
            .iter()
            .any(|record| record.payload().windows(15).any(|w| w == b"api.example.com"))
    );
    for record in &stored {
        assert!(!leaks(record.payload()));
        assert!(!leaks(format!("{record:?}").as_bytes()));
    }
    for text in &rendered {
        assert!(!leaks(text.as_bytes()), "{text}");
    }
    assert!(
        !captured.events().is_empty(),
        "the failing sink must have produced log lines to check"
    );
    std::hint::black_box(&ambient);
}
