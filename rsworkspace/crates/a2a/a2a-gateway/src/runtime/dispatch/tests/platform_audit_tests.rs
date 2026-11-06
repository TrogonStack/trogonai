//! The platform audit path through dispatch: what is recorded for each decision,
//! when a lost record refuses the request, and what a refusal does not do.

use std::sync::Arc;

use async_nats::jetstream::stream::Config as StreamConfig;
use buffa::{EnumValue, Message};
use serde_json::{Value, json};
use trogon_audit::{ChainAnchor, MemorySink, SealedRecord, SinkError, verify_chain_range};
use trogon_nats::test_support::JetStreamTestServer;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{AuditDecision, AuditRecord};

use super::RejectingGate;
use super::fixture::{
    AUDIT_PROJECT, DispatchFixture, RecordingOwner, RecordingTier1, TestResult, audit_writer_id, receive, request,
};
use crate::policy::spicedb_tier1::Tier1AuthorizeOutcome;
use crate::policy::tier2::DenyAllTier2Evaluator;
use crate::policy::tier3_redaction::Tier3RedactionDecision;
use crate::policy::wasmtime_substrate::{Tier2State, WasmtimeSubstrate};
use a2a_redaction::{SkillId, WasmBundlePath};

fn decoded(record: &SealedRecord) -> AuditRecord {
    AuditRecord::decode_from_slice(record.payload()).expect("a record the writer sealed decodes")
}

/// The decisions the sink stored, after checking that everything it stored is
/// one gap-free chain opened by the writer's chain reset, which is left out.
fn decisions(sink: &MemorySink) -> Vec<SealedRecord> {
    let stored = sink.stored();
    let Some(genesis) = stored.first() else {
        return Vec::new();
    };
    let anchor = ChainAnchor::Genesis {
        writer: audit_writer_id(),
        chain: genesis.chain(),
    };
    let verified = verify_chain_range(anchor, sink.stored_records()).expect("one gap-free chain");
    assert_eq!(verified.records(), stored.len());
    stored.into_iter().skip(1).collect()
}

fn only(sink: &MemorySink) -> SealedRecord {
    let mut stored = decisions(sink);
    assert_eq!(stored.len(), 1, "stored: {}", stored.len());
    stored.remove(0)
}

fn target(record: &SealedRecord) -> String {
    match decoded(record).detail {
        Some(Detail::GatewayDecision(decision)) => decision.target,
        other => panic!("not a decision: {other:?}"),
    }
}

fn subject(outcome: &str) -> String {
    format!("platform.v1.audit.decision.{AUDIT_PROJECT}.{outcome}")
}

#[tokio::test]
async fn an_allow_that_acts_is_recorded_before_the_agent_sees_the_request() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    fixture.dispatch(request("message.send", json!({}))).await;
    receive(&mut fixture.agents).await?;
    let record = only(&sink);
    assert_eq!(record.subject(), subject("allow"));
    let message = decoded(&record);
    assert_eq!(
        message.attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW)
    );
    assert_eq!(message.request_id.as_deref(), Some("request-7"));
    assert!(format!("{:?}", message.actor).contains("alice"));
    Ok(())
}

#[tokio::test]
async fn an_allow_that_acts_and_cannot_be_recorded_is_refused_and_never_forwarded() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    sink.fail_all(SinkError::Unreachable);
    fixture.dispatch(request("message.send", json!({}))).await;
    let body = fixture.denied(-32804, json!("request-7")).await?;
    assert_eq!(body["error"]["data"]["reason"], "audit_unavailable");
    assert!(decisions(&sink).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_refusal_for_a_lost_record_is_published_on_the_legacy_envelope_too() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    sink.fail_all(SinkError::Timeout);
    fixture.dispatch(request("message.send", json!({}))).await;
    fixture.denied(-32804, json!("request-7")).await?;
    let audit = fixture.audit("err").await?;
    assert_eq!(audit["code"], -32804);
    Ok(())
}

#[tokio::test]
async fn a_read_declared_allow_is_forwarded_even_when_the_record_cannot_be_taken() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, writer) = fixture.enable_memory_audit();
    sink.fail_all(SinkError::Unreachable);
    fixture.dispatch(request("tasks.get", json!({"id": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    assert!(decisions(&sink).is_empty());

    sink.recover();
    writer.drain_once().await;
    assert_eq!(only(&sink).subject(), subject("allow"));
    Ok(())
}

#[tokio::test]
async fn a_deny_is_replied_whatever_the_stream_does_and_is_stored_when_it_returns() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, writer) = fixture.enable_memory_audit();
    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Denied));
    sink.fail_all(SinkError::Unreachable);
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    fixture.denied(-32801, json!("request-7")).await?;
    assert!(decisions(&sink).is_empty());

    sink.recover();
    writer.drain_once().await;
    let record = only(&sink);
    assert_eq!(record.subject(), subject("deny"));
    let message = decoded(&record);
    assert_eq!(message.attribution.reason_code.as_deref(), Some("tier1_spicedb_denied"));
    assert_eq!(
        message.attribution.rule_ids,
        vec!["gateway.tier1.spicedb_denied".to_owned()]
    );
    Ok(())
}

#[tokio::test]
async fn every_tier_records_its_own_reason_when_it_denies() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();

    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Denied));
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    fixture.denied(-32801, json!("request-7")).await?;
    fixture.tier1 = Arc::new(crate::policy::spicedb_tier1::NoopSpiceDbTier1Gate);

    let directory = tempfile::tempdir()?;
    fixture.policy.substrate = Some(Arc::new(WasmtimeSubstrate::try_new_with_tier2(
        WasmBundlePath::new(directory.path()),
        Tier2State::Active(Box::new(DenyAllTier2Evaluator)),
        None,
    )?));
    fixture.dispatch(request("message.send", json!({}))).await;
    fixture.denied(-32801, json!("request-7")).await?;
    fixture.policy.substrate = None;

    for (decision, code) in [
        (
            Tier3RedactionDecision::Refuse {
                reason: crate::policy::tier3_redaction::Tier3RefusalReason::UnauthorizedDataCategory,
                rule: SkillId::new("pii")?,
            },
            -32802,
        ),
        (
            Tier3RedactionDecision::Error {
                kind: crate::policy::tier3_redaction::Tier3EngineError::WasmTrap,
                rule: SkillId::new("pii")?,
            },
            -32801,
        ),
    ] {
        fixture.policy.tier3_gate = Arc::new(RejectingGate { decision });
        fixture.dispatch(request("message.send", json!({}))).await;
        fixture.denied(code, json!("request-7")).await?;
    }

    let reasons: Vec<Option<String>> = decisions(&sink)
        .iter()
        .map(|record| decoded(record).attribution.reason_code.clone())
        .collect();
    assert_eq!(
        reasons,
        vec![
            Some("tier1_spicedb_denied".to_owned()),
            Some("tier2_denied".to_owned()),
            Some("tier3_refused".to_owned()),
            Some("tier3_engine_error".to_owned()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn an_unparseable_payload_is_denied_and_recorded() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    let mut message = request("message.send", json!({}));
    message.payload = bytes::Bytes::from_static(b"{");
    fixture.dispatch(message).await;
    fixture.denied(-32801, Value::Null).await?;
    let message = decoded(&only(&sink));
    assert_eq!(
        message.attribution.reason_code.as_deref(),
        Some("tier3_payload_invalid")
    );
    Ok(())
}

#[tokio::test]
async fn nothing_is_recorded_on_the_platform_path_when_it_is_off() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    assert!(fixture.platform_audit.is_none());
    fixture.dispatch(request("message.send", json!({}))).await;
    receive(&mut fixture.agents).await?;
    fixture.audit("ok").await?;
    Ok(())
}

#[tokio::test]
async fn no_owner_tuple_is_written_when_the_allow_record_cannot_be_stored() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    let owner = Arc::new(RecordingOwner::default());
    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.owner = Some(owner.clone());
    sink.fail_all(SinkError::Unreachable);
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    fixture.denied(-32804, json!("request-7")).await?;
    assert!(owner.calls.lock().expect("owner calls").is_empty());
    Ok(())
}

#[tokio::test]
async fn the_owner_tuple_is_written_once_the_allow_record_is_stored() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    let owner = Arc::new(RecordingOwner::default());
    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.owner = Some(owner.clone());
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    assert_eq!(only(&sink).subject(), subject("allow"));
    let calls = owner.calls.lock().expect("owner calls").clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].resource_id.as_str(), "bot:task-1");
    Ok(())
}

async fn audit_stream(
    fixture: &DispatchFixture<JetStreamTestServer>,
) -> TestResult<async_nats::jetstream::stream::Stream> {
    let jetstream = async_nats::jetstream::new(fixture.client.clone());
    Ok(jetstream
        .create_stream(StreamConfig {
            name: "AUDIT".into(),
            subjects: vec!["platform.v1.audit.>".into()],
            ..Default::default()
        })
        .await?)
}

#[tokio::test]
async fn jetstream_stores_the_allow_and_the_deny_under_their_decision_subjects() -> TestResult {
    let mut fixture = DispatchFixture::streaming().await?;
    let stream = audit_stream(&fixture).await?;
    fixture.enable_jetstream_audit();

    fixture.dispatch(request("message.send", json!({}))).await;
    receive(&mut fixture.agents).await?;
    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Denied));
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    fixture.denied(-32801, json!("request-7")).await?;

    // The allow was acknowledged before the agent saw the request; the deny is
    // written on the dispatch task and acknowledged by the time it returns.
    let allow = stream
        .get_last_raw_message_by_subject(&subject("allow"))
        .await
        .expect("allow stored");
    let allow = AuditRecord::decode_from_slice(&allow.payload).expect("allow decodes");
    assert_eq!(
        allow.attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW)
    );
    let deny = stream
        .get_last_raw_message_by_subject(&subject("deny"))
        .await
        .expect("deny stored");
    let deny = AuditRecord::decode_from_slice(&deny.payload).expect("deny decodes");
    assert_eq!(deny.attribution.reason_code.as_deref(), Some("tier1_spicedb_denied"));
    Ok(())
}

#[tokio::test]
async fn jetstream_without_an_audit_stream_refuses_what_must_be_recorded() -> TestResult {
    let mut fixture = DispatchFixture::streaming().await?;
    fixture.enable_jetstream_audit();
    fixture.dispatch(request("message.send", json!({}))).await;
    let body = fixture.denied(-32804, json!("request-7")).await?;
    assert_eq!(body["error"]["data"]["reason"], "audit_unavailable");
    Ok(())
}

#[tokio::test]
async fn a_decision_names_the_json_rpc_method_not_its_subject_terminal() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let (sink, _writer) = fixture.enable_memory_audit();
    fixture.dispatch(request("push.set", json!({"taskId": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    assert_eq!(target(&only(&sink)), "tasks/pushNotificationConfig/set@bot");

    let (sink, writer) = fixture.enable_memory_audit();
    fixture.dispatch(request("card", json!({}))).await;
    receive(&mut fixture.agents).await?;
    writer.drain_once().await;
    assert_eq!(target(&only(&sink)), "agent/card@bot");
    Ok(())
}
