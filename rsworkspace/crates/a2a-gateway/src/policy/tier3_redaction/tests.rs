use std::collections::BTreeMap;

use a2a_nats::ingress_gateway_policy_denied_response_bytes;
use a2a_nats::ingress_gateway_tier3_refused_response_bytes;
use a2a_redaction::{RedactionError, SkillId, TIER3_REFUSE_SENTINEL};
use trogon_std::env::InMemoryEnv;

use super::context::Tier3EvaluationContext;
use super::decision::{Tier3EngineError, Tier3RedactionDecision, Tier3RefusalReason};
use super::gate::{NoopTier3RedactionGate, Tier3RedactionGate};
use super::manifest::Tier3SkillManifest;
use super::real_gate::{MockTier3PartInvoker, RealTier3RedactionGate};
use super::rewrite::RewriteKind;
use crate::policy::tier3_redaction::gateway_tier3_redaction_enabled;

fn sample_payload() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-1",
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{ "text": "secret@example.com" }]
            }
        }
    })
}

fn manifest_for(skill: &str, path: &str) -> Tier3SkillManifest {
    Tier3SkillManifest::new(SkillId::new(skill), path, RewriteKind::Masked)
}

#[test]
fn noop_gate_returns_allow_with_empty_rewrites() {
    let gate = NoopTier3RedactionGate;
    let mut ctx = Tier3EvaluationContext::new(
        "message/send",
        Some("caller-1".into()),
        sample_payload(),
        BTreeMap::new(),
    );
    assert_eq!(
        gate.redact(&mut ctx),
        Tier3RedactionDecision::Allow {
            rewrites: Vec::new()
        }
    );
}

#[test]
fn gateway_tier3_redaction_env_defaults_off() {
    let env = InMemoryEnv::new();
    assert!(!gateway_tier3_redaction_enabled(&env));
}

#[test]
fn real_gate_records_rewrite_on_mock_host() {
    let skill = SkillId::new("pii-email");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("pii-email", "$.params.message.parts[0].text"));

    let invoker = MockTier3PartInvoker::with_output(skill.clone(), br#""[REDACTED]""#.to_vec());
    let gate = RealTier3RedactionGate::new(invoker);

    let mut ctx = Tier3EvaluationContext::new(
        "message/send",
        Some("caller-1".into()),
        sample_payload(),
        manifests,
    );

    match gate.redact(&mut ctx) {
        Tier3RedactionDecision::Allow { rewrites } => {
            assert_eq!(rewrites.len(), 1);
            assert_eq!(rewrites[0].skill_id().as_str(), "pii-email");
            assert_eq!(
                value_at_path(ctx.payload(), "$.params.message.parts[0].text"),
                Some(&serde_json::Value::String("[REDACTED]".into()))
            );
        }
        other => panic!("expected allow, got {other:?}"),
    }
}

#[test]
fn real_gate_refusal_sentinel_returns_refuse() {
    let skill = SkillId::new("deny-part");
    let mut manifests = BTreeMap::new();
    manifests.insert(
        skill.clone(),
        manifest_for("deny-part", "/params/message/parts/0/text"),
    );

    let mut sentinel = TIER3_REFUSE_SENTINEL.to_vec();
    sentinel.extend_from_slice(b":UnauthorizedDataCategory");
    let invoker = MockTier3PartInvoker::with_output(skill.clone(), sentinel);
    let gate = RealTier3RedactionGate::new(invoker);

    let mut ctx = Tier3EvaluationContext::new(
        "message/send",
        Some("caller-1".into()),
        sample_payload(),
        manifests,
    );

    assert_eq!(
        gate.redact(&mut ctx),
        Tier3RedactionDecision::Refuse {
            reason: Tier3RefusalReason::UnauthorizedDataCategory,
            rule: skill,
        }
    );
}

#[test]
fn real_gate_trap_maps_to_error() {
    let skill = SkillId::new("trap");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("trap", "/params/message/parts/0/text"));

    let invoker = MockTier3PartInvoker::with_error(skill.clone(), RedactionError::WasmCall("trap".into()));
    let gate = RealTier3RedactionGate::new(invoker);

    let mut ctx = Tier3EvaluationContext::new(
        "message/send",
        Some("caller-1".into()),
        sample_payload(),
        manifests,
    );

    assert_eq!(
        gate.redact(&mut ctx),
        Tier3RedactionDecision::Error {
            rule: skill,
            kind: Tier3EngineError::WasmTrap,
        }
    );
}

#[test]
fn tier3_refused_response_uses_32802_and_rule_data() {
    let payload = br#"{"jsonrpc":"2.0","id":"x","method":"message/send","params":{}}"#;
    let bytes = ingress_gateway_tier3_refused_response_bytes(payload, "tier-3 skill refused part", "deny-part")
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], -32_802);
    assert_eq!(value["error"]["data"]["rule"], "deny-part");
}

#[test]
fn tier3_engine_error_response_uses_32801() {
    let payload = br#"{"jsonrpc":"2.0","id":"x","method":"message/send","params":{}}"#;
    let bytes = ingress_gateway_policy_denied_response_bytes(payload, "tier-3 redaction engine error").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], -32_801);
}

fn value_at_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    super::json_path::value_at_path(root, path)
}

#[test]
fn dispatch_path_tier3_allow_then_forward_payload_bytes() {
    let skill = SkillId::new("mask");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("mask", "/params/message/parts/0/text"));

    let invoker = MockTier3PartInvoker::with_output(skill, br#""***""#.to_vec());
    let gate = RealTier3RedactionGate::new(invoker);

    let raw = br#"{"jsonrpc":"2.0","id":"1","method":"message/send","params":{"message":{"parts":[{"text":"open"}]}}}"#;
    let mut ctx = Tier3EvaluationContext::from_json_rpc_payload("message/send", Some("caller".into()), raw, manifests);

    let decision = gate.redact(&mut ctx);
    assert!(decision.is_allow());
    let forwarded = ctx.into_payload_bytes().unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&forwarded).unwrap();
    assert_eq!(parsed["params"]["message"]["parts"][0]["text"], "***");
}
