use std::time::Instant;

use a2a_nats::A2aPrefix;
use a2a_nats::agent_id::A2aAgentId;
use async_nats::Subject;
use bytes::Bytes;
use trogon_nats::mocks::MockNatsClient;

use super::*;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").expect("a2a is a valid prefix")
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("planner").expect("nats-safe test agent id")
}

fn well_formed_payload() -> Bytes {
    Bytes::from_static(br#"{"jsonrpc":"2.0","id":"req-1","method":"message/send","params":{}}"#)
}

fn denial_body() -> Bytes {
    Bytes::from_static(br#"{"jsonrpc":"2.0","id":"req-1","error":{"code":-32801,"message":"denied"}}"#)
}

async fn wait_for(nats: &MockNatsClient, expected: usize) {
    for _ in 0..50 {
        if nats.published_messages().len() >= expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!(
        "expected {expected} published messages, got {}",
        nats.published_messages().len()
    );
}

fn ctx<'a>(
    prefix: &'a A2aPrefix,
    agent: &'a A2aAgentId,
    payload: &'a Bytes,
    caller_source: &'a Option<String>,
    audit_enabled: bool,
) -> Tier1DenialCtx<'a> {
    Tier1DenialCtx {
        a2a_prefix: prefix,
        agent_id: agent,
        method_slashes: "message/send",
        payload,
        trace_id: "trace-1",
        audit_enabled,
        started_wall_ms: 1_700_000_000_000,
        started_mono: Instant::now(),
        audit_caller_id: "user/alice",
        audit_caller_source: caller_source,
    }
}

#[tokio::test]
async fn deny_tier1_writes_reply_and_audit_when_publishing_enabled() {
    // Happy path: a pre-built denial body and audit enabled. Both
    // the caller-inbox reply and the audit envelope must show up
    // on the mock NATS client.
    let nats = MockNatsClient::new();
    let pre = prefix();
    let agent = agent();
    let payload = well_formed_payload();
    let caller_source = Some("jwt-mint".to_owned());
    deny_tier1(
        &nats,
        Subject::from("_INBOX.reply"),
        denial_body(),
        ctx(&pre, &agent, &payload, &caller_source, true),
        "denied",
    )
    .await;
    wait_for(&nats, 2).await;
    let subjects = nats.published_messages();
    assert!(subjects.contains(&"_INBOX.reply".to_owned()));
    assert!(subjects.iter().any(|s| s == "a2a.audit.err.message.send"));
}

#[tokio::test]
async fn deny_tier1_skips_audit_when_publishing_disabled() {
    // Audit-disabled deployments must still get the caller reply
    // but pay zero audit-stream traffic.
    let nats = MockNatsClient::new();
    let pre = prefix();
    let agent = agent();
    let payload = well_formed_payload();
    let caller_source: Option<String> = None;
    deny_tier1(
        &nats,
        Subject::from("_INBOX.reply"),
        denial_body(),
        ctx(&pre, &agent, &payload, &caller_source, false),
        "denied",
    )
    .await;
    wait_for(&nats, 1).await;
    let subjects = nats.published_messages();
    assert_eq!(subjects, vec!["_INBOX.reply".to_owned()]);
}

#[tokio::test]
async fn deny_tier1_audit_envelope_stamps_caller_pair_and_tier1_decision() {
    let nats = MockNatsClient::new();
    let pre = prefix();
    let agent = agent();
    let payload = well_formed_payload();
    let caller_source = Some("jwt-mint".to_owned());
    deny_tier1(
        &nats,
        Subject::from("_INBOX.reply"),
        denial_body(),
        ctx(&pre, &agent, &payload, &caller_source, true),
        "denied",
    )
    .await;
    wait_for(&nats, 2).await;
    let subjects = nats.published_messages();
    let payloads = nats.published_payloads();
    let audit_payload = subjects
        .iter()
        .zip(payloads.iter())
        .find_map(|(s, p)| (s != "_INBOX.reply").then(|| p.clone()))
        .expect("audit envelope published");
    let json: serde_json::Value = serde_json::from_slice(&audit_payload).expect("audit envelope is json");
    assert_eq!(json["caller_id"], "user/alice");
    assert_eq!(json["caller_source"], "jwt-mint");
    assert_eq!(json["tier1_decision"], "deny");
    assert_eq!(json["trace_id"], "trace-1");
    assert_eq!(json["outcome"], "err");
    assert_eq!(json["code"], -32_801);
    assert_eq!(json["rules_fired"], serde_json::json!(["gateway.tier1.spicedb_denied"]));
}
