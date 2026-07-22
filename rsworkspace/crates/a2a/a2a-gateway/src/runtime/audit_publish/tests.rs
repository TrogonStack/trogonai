use a2a_nats::A2aPrefix;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::audit::envelope::{AuditEnvelope, AuditEnvelopeFields, AuditOutcome};
use trogon_nats::mocks::MockNatsClient;

use super::*;

fn agent() -> A2aAgentId {
    A2aAgentId::new("planner").expect("nats-safe test agent id")
}

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").expect("a2a is a valid prefix")
}

fn ok_envelope() -> AuditEnvelope {
    AuditEnvelope::new(
        &agent(),
        "message/send",
        Some("req-1".to_owned()),
        0,
        0,
        AuditOutcome::Ok,
        None,
        AuditEnvelopeFields::default(),
    )
}

fn err_envelope() -> AuditEnvelope {
    AuditEnvelope::new(
        &agent(),
        "tasks/get",
        Some("req-2".to_owned()),
        0,
        0,
        AuditOutcome::Err {
            code: -32_801,
            message: "denied".into(),
        },
        None,
        AuditEnvelopeFields::default(),
    )
}

async fn wait_for_publish(nats: &MockNatsClient, expected: usize) {
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

#[tokio::test]
async fn disabled_publish_is_no_op_and_does_not_spawn() {
    // The `enabled = false` short-circuit is what lets deployments
    // without an audit stream avoid paying any publish cost. Verify
    // the helper does not even touch the client when disabled.
    let nats = MockNatsClient::new();
    spawn_gateway_audit_publish(false, nats.clone(), prefix(), agent(), ok_envelope());
    // No wait_for_publish here -- there's nothing to wait for.
    // Give any errantly-spawned task a chance to run before asserting.
    tokio::task::yield_now().await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn enabled_publish_targets_ok_audit_subject() {
    let nats = MockNatsClient::new();
    spawn_gateway_audit_publish(true, nats.clone(), prefix(), agent(), ok_envelope());
    wait_for_publish(&nats, 1).await;
    let subjects = nats.published_messages();
    assert_eq!(subjects, vec!["a2a.audit.ok.message.send".to_owned()]);
}

#[tokio::test]
async fn enabled_publish_targets_err_audit_subject() {
    // Outcome::Err must route to the `.err.` subject so consumers
    // can subscribe to denials independently of allow-throughs.
    let nats = MockNatsClient::new();
    spawn_gateway_audit_publish(true, nats.clone(), prefix(), agent(), err_envelope());
    wait_for_publish(&nats, 1).await;
    let subjects = nats.published_messages();
    assert_eq!(subjects, vec!["a2a.audit.err.tasks.get".to_owned()]);
}

#[tokio::test]
async fn enabled_publish_serializes_envelope_as_json_payload() {
    let nats = MockNatsClient::new();
    spawn_gateway_audit_publish(true, nats.clone(), prefix(), agent(), ok_envelope());
    wait_for_publish(&nats, 1).await;
    let payload = nats.published_payloads().pop().expect("one payload");
    let json: serde_json::Value = serde_json::from_slice(&payload).expect("payload is json");
    assert_eq!(json["method"], "message/send");
    assert_eq!(json["req_id"], "req-1");
    assert_eq!(json["agent_id"], "planner");
}
