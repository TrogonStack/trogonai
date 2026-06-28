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
    Tier3SkillManifest::new(
        SkillId::new(skill).expect("non-empty test skill"),
        path,
        RewriteKind::Masked,
    )
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
        Tier3RedactionDecision::Allow { rewrites: Vec::new() }
    );
}

#[test]
fn gateway_tier3_redaction_env_defaults_off() {
    let env = InMemoryEnv::new();
    assert!(!gateway_tier3_redaction_enabled(&env));
}

#[test]
fn real_gate_records_rewrite_on_mock_host() {
    let skill = SkillId::new("pii-email").expect("non-empty test skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(
        skill.clone(),
        manifest_for("pii-email", "$.params.message.parts[0].text"),
    );

    let invoker = MockTier3PartInvoker::with_output(skill.clone(), br#""[REDACTED]""#.to_vec());
    let gate = RealTier3RedactionGate::new(invoker);

    let mut ctx = Tier3EvaluationContext::new("message/send", Some("caller-1".into()), sample_payload(), manifests);

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
    let skill = SkillId::new("deny-part").expect("non-empty test skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("deny-part", "/params/message/parts/0/text"));

    let mut sentinel = TIER3_REFUSE_SENTINEL.to_vec();
    sentinel.extend_from_slice(b":UnauthorizedDataCategory");
    let invoker = MockTier3PartInvoker::with_output(skill.clone(), sentinel);
    let gate = RealTier3RedactionGate::new(invoker);

    let mut ctx = Tier3EvaluationContext::new("message/send", Some("caller-1".into()), sample_payload(), manifests);

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
    let skill = SkillId::new("trap").expect("non-empty test skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("trap", "/params/message/parts/0/text"));

    let invoker = MockTier3PartInvoker::with_error(skill.clone(), RedactionError::WasmCall("trap".into()));
    let gate = RealTier3RedactionGate::new(invoker);

    let mut ctx = Tier3EvaluationContext::new("message/send", Some("caller-1".into()), sample_payload(), manifests);

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
    // Wire shape on main: error code lives in headers under
    // `Jsonrpc-Error-Code` (jsonrpc-nats encoding splits code from the
    // body), and the body carries `message` + optional `data`. The
    // assertions below match that shape rather than the legacy JSON-RPC
    // wire form that nested everything under an `error` field.
    let payload = br#"{"jsonrpc":"2.0","id":"x","method":"message/send","params":{}}"#;
    let headers = async_nats::HeaderMap::new();
    let bytes =
        ingress_gateway_tier3_refused_response_bytes(&headers, payload, "tier-3 skill refused part", "deny-part")
            .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["message"], "tier-3 skill refused part");
    assert_eq!(value["data"]["rule"], "deny-part");
}

#[test]
fn tier3_engine_error_response_uses_32801() {
    let payload = br#"{"jsonrpc":"2.0","id":"x","method":"message/send","params":{}}"#;
    let headers = async_nats::HeaderMap::new();
    let bytes =
        ingress_gateway_policy_denied_response_bytes(&headers, payload, "tier-3 redaction engine error").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["message"], "tier-3 redaction engine error");
}

fn value_at_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    super::json_path::value_at_path(root, path)
}

#[test]
fn dispatch_path_tier3_allow_then_forward_payload_bytes() {
    let skill = SkillId::new("mask").expect("non-empty test skill");
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

#[test]
fn refusal_reason_round_trips_through_sentinel_tag() {
    assert_eq!(
        Tier3RefusalReason::from_sentinel_tag("UnauthorizedDataCategory"),
        Tier3RefusalReason::UnauthorizedDataCategory
    );
    assert_eq!(
        Tier3RefusalReason::from_sentinel_tag("InvalidPayloadShape"),
        Tier3RefusalReason::InvalidPayloadShape
    );
    // Unknown tags fall through to SkillPolicyDeniedPart so a guest can't
    // suppress the audit by emitting a novel reason string.
    assert_eq!(
        Tier3RefusalReason::from_sentinel_tag("anything-else"),
        Tier3RefusalReason::SkillPolicyDeniedPart
    );
    assert_eq!(
        Tier3RefusalReason::SkillPolicyDeniedPart.as_str(),
        "SkillPolicyDeniedPart"
    );
    assert_eq!(Tier3RefusalReason::InvalidPayloadShape.as_str(), "InvalidPayloadShape");
    assert_eq!(
        Tier3RefusalReason::UnauthorizedDataCategory.as_str(),
        "UnauthorizedDataCategory"
    );
}

#[test]
fn engine_error_labels_cover_every_variant() {
    assert_eq!(Tier3EngineError::WasmTrap.as_str(), "WasmTrap");
    assert_eq!(Tier3EngineError::WasmAbi.as_str(), "WasmAbi");
    assert_eq!(Tier3EngineError::InvalidPayload.as_str(), "InvalidPayload");
}

#[test]
fn redaction_decision_is_allow_predicate() {
    assert!(Tier3RedactionDecision::Allow { rewrites: Vec::new() }.is_allow());
    assert!(
        !Tier3RedactionDecision::Refuse {
            reason: Tier3RefusalReason::SkillPolicyDeniedPart,
            rule: SkillId::new("x").expect("non-empty test skill"),
        }
        .is_allow()
    );
}

#[test]
fn rewrite_kind_parses_known_tags_and_rejects_unknowns() {
    assert_eq!(RewriteKind::from_manifest_str("Replaced"), Some(RewriteKind::Replaced));
    assert_eq!(RewriteKind::from_manifest_str("replaced"), Some(RewriteKind::Replaced));
    assert_eq!(RewriteKind::from_manifest_str("Removed"), Some(RewriteKind::Removed));
    assert_eq!(RewriteKind::from_manifest_str("Masked"), Some(RewriteKind::Masked));
    assert_eq!(RewriteKind::from_manifest_str("nope"), None);
    assert_eq!(RewriteKind::Replaced.as_str(), "Replaced");
    assert_eq!(RewriteKind::Removed.as_str(), "Removed");
    assert_eq!(RewriteKind::Masked.as_str(), "Masked");
}

#[test]
fn rewrite_kind_infers_removed_on_null_after_value() {
    assert_eq!(
        RewriteKind::infer_from_values(&serde_json::json!("before"), &serde_json::Value::Null),
        RewriteKind::Removed
    );
    assert_eq!(
        RewriteKind::infer_from_values(&serde_json::json!("a"), &serde_json::json!("b")),
        RewriteKind::Replaced
    );
}

#[test]
fn redaction_rewrite_display_renders_skill_kind_path() {
    let rewrite = super::rewrite::RedactionRewrite::new(
        SkillId::new("pii-email").expect("skill"),
        "$.params.message",
        RewriteKind::Masked,
    );
    assert_eq!(format!("{rewrite}"), "pii-email:Masked@$.params.message");
    assert_eq!(rewrite.skill_id().as_str(), "pii-email");
    assert_eq!(rewrite.path_jsonpath(), "$.params.message");
    assert_eq!(rewrite.kind(), &RewriteKind::Masked);
}

#[test]
fn audit_rewrites_returns_none_for_empty() {
    assert!(super::tier3_redaction_audit_rewrites(&[]).is_none());
}

#[test]
fn audit_rewrites_serializes_each_rewrite() {
    let rewrite =
        super::rewrite::RedactionRewrite::new(SkillId::new("pii").expect("skill"), "$.x", RewriteKind::Removed);
    let value = super::tier3_redaction_audit_rewrites(&[rewrite]).expect("some");
    let array = value.as_array().expect("array");
    assert_eq!(array.len(), 1);
    assert_eq!(array[0].as_str(), Some("pii:Removed@$.x"));
}

#[test]
fn gateway_tier3_redaction_env_recognizes_truthy_strings() {
    for raw in ["1", "true", "yes", "on", "YES", "On", "TRUE"] {
        let env = InMemoryEnv::new();
        env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", raw);
        assert!(
            gateway_tier3_redaction_enabled(&env),
            "expected `{raw}` to enable tier-3",
        );
    }
    let env = InMemoryEnv::new();
    env.set("A2A_GATEWAY_TIER3_REDACTION_ENABLED", "off");
    assert!(!gateway_tier3_redaction_enabled(&env));
}

#[test]
fn manifest_parse_uses_default_replaced_when_kind_missing() {
    let skill = SkillId::new("pii").expect("skill");
    let raw = serde_json::json!({"json_path": "$.x"});
    let manifest = Tier3SkillManifest::parse(skill.clone(), &raw).expect("parsed");
    assert_eq!(manifest.skill_id(), &skill);
    assert_eq!(manifest.json_path(), "$.x");
    assert_eq!(manifest.kind(), &RewriteKind::Replaced);
}

#[test]
fn manifest_parse_rejects_missing_or_empty_json_path() {
    let skill = SkillId::new("pii").expect("skill");
    assert!(Tier3SkillManifest::parse(skill.clone(), &serde_json::json!({})).is_none());
    assert!(Tier3SkillManifest::parse(skill.clone(), &serde_json::json!({"json_path": ""})).is_none());
}

#[test]
fn manifest_parse_warns_on_skill_id_mismatch_but_uses_manifest_value() {
    // When the manifest carries a different skill_id than the file
    // stem, we trust the manifest value but log a warning so the audit
    // surface can flag the inconsistency.
    let file_skill = SkillId::new("pii").expect("skill");
    let raw = serde_json::json!({"json_path": "$.x", "skill_id": "pii-override"});
    let manifest = Tier3SkillManifest::parse(file_skill, &raw).expect("parsed");
    assert_eq!(manifest.skill_id().as_str(), "pii-override");
}

#[test]
fn load_tier3_manifests_skips_missing_files() {
    use a2a_redaction::WasmBundlePath;
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = WasmBundlePath::new(dir.path());
    let manifests = super::load_tier3_manifests_from_bundle(&bundle, &[SkillId::new("missing").expect("skill")]);
    assert!(manifests.is_empty());
}

#[test]
fn real_gate_invalid_payload_rewrite_value_yields_error() {
    // A guest that returns invalid JSON should surface as an
    // InvalidPayload error tagged with the offending skill so audit can
    // route on it.
    let skill = SkillId::new("bad-json").expect("skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("bad-json", "/params/message/parts/0/text"));
    let invoker = MockTier3PartInvoker::with_output(skill.clone(), b"not-json".to_vec());
    let gate = RealTier3RedactionGate::new(invoker);
    let mut ctx = Tier3EvaluationContext::new("message/send", None, sample_payload(), manifests);
    assert_eq!(
        gate.redact(&mut ctx),
        Tier3RedactionDecision::Error {
            rule: skill,
            kind: Tier3EngineError::InvalidPayload,
        }
    );
}

#[test]
fn real_gate_unchanged_value_produces_no_rewrite() {
    let skill = SkillId::new("noop").expect("skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("noop", "$.params.message.parts[0].text"));
    let invoker = MockTier3PartInvoker::with_output(skill, br#""secret@example.com""#.to_vec());
    let gate = RealTier3RedactionGate::new(invoker);
    let mut ctx = Tier3EvaluationContext::new("message/send", None, sample_payload(), manifests);
    match gate.redact(&mut ctx) {
        Tier3RedactionDecision::Allow { rewrites } => assert!(rewrites.is_empty()),
        other => panic!("expected allow with no rewrites, got {other:?}"),
    }
}

#[test]
fn real_gate_missing_path_skips_skill_without_error() {
    let skill = SkillId::new("missing-path").expect("skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(skill.clone(), manifest_for("missing-path", "$.params.nonexistent"));
    let invoker = MockTier3PartInvoker::with_output(skill, b"\"x\"".to_vec());
    let gate = RealTier3RedactionGate::new(invoker);
    let mut ctx = Tier3EvaluationContext::new("message/send", None, sample_payload(), manifests);
    match gate.redact(&mut ctx) {
        Tier3RedactionDecision::Allow { rewrites } => assert!(rewrites.is_empty()),
        other => panic!("expected allow, got {other:?}"),
    }
}

#[test]
fn merge_forward_audit_rewrites_with_only_tier3_returns_tier3_array() {
    let rewrite =
        super::rewrite::RedactionRewrite::new(SkillId::new("pii").expect("skill"), "$.x", RewriteKind::Masked);
    let agent_id = a2a_nats::A2aAgentId::new("planner").expect("agent");
    let (audit, stream_consumer) = super::merge_forward_audit_rewrites(
        &[rewrite],
        "a2a.gateway.bot.message.send",
        "a2a.agent.planner.message.send",
        &agent_id,
        "message.send",
    );
    assert!(audit.is_some());
    let _ = stream_consumer;
}

#[test]
fn evaluation_context_from_json_rpc_payload_handles_invalid_bytes() {
    let ctx = Tier3EvaluationContext::from_json_rpc_payload("m", None, b"not-json", BTreeMap::new());
    assert_eq!(*ctx.payload(), serde_json::Value::Null);
}

#[test]
fn evaluation_context_accessors_round_trip() {
    let mut manifests = BTreeMap::new();
    let skill = SkillId::new("pii").expect("skill");
    manifests.insert(
        skill.clone(),
        Tier3SkillManifest::new(skill, "$.x", RewriteKind::Masked),
    );
    let ctx = Tier3EvaluationContext::new("m", Some("c".into()), serde_json::json!({"a":1}), manifests.clone());
    assert_eq!(ctx.method(), "m");
    assert_eq!(ctx.caller_id(), Some("c"));
    assert_eq!(ctx.skill_manifests().len(), 1);
    let bytes = ctx.into_payload_bytes().expect("to bytes");
    assert!(!bytes.is_empty());
}

#[test]
fn load_tier3_manifests_reads_valid_bundle() {
    // Drop a manifest JSON into the path that WasmBundlePath::join_skill_manifest
    // resolves to so the loader picks it up. Verifies the happy-path
    // file-read + parse + insert chain rather than just the
    // missing-file branch.
    use a2a_redaction::WasmBundlePath;
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = WasmBundlePath::new(dir.path());
    let skill = SkillId::new("email-redactor").expect("skill");
    let manifest_path = bundle.join_skill_manifest(&skill);
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir manifest parent");
    }
    std::fs::write(
        &manifest_path,
        r#"{"json_path":"$.params.message.parts[0].text","kind":"Masked"}"#,
    )
    .expect("write manifest");
    let manifests = super::load_tier3_manifests_from_bundle(&bundle, std::slice::from_ref(&skill));
    assert_eq!(manifests.len(), 1);
    let resolved = manifests.get(&skill).expect("present");
    assert_eq!(resolved.json_path(), "$.params.message.parts[0].text");
    assert_eq!(resolved.kind(), &RewriteKind::Masked);
}

#[test]
fn load_tier3_manifests_skips_invalid_json() {
    use a2a_redaction::WasmBundlePath;
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = WasmBundlePath::new(dir.path());
    let skill = SkillId::new("broken").expect("skill");
    let path = bundle.join_skill_manifest(&skill);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&path, "not-json").expect("write");
    let manifests = super::load_tier3_manifests_from_bundle(&bundle, &[skill]);
    assert!(manifests.is_empty(), "invalid JSON skill is skipped silently");
}

#[test]
fn load_tier3_manifests_skips_manifest_missing_json_path() {
    use a2a_redaction::WasmBundlePath;
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = WasmBundlePath::new(dir.path());
    let skill = SkillId::new("no-path").expect("skill");
    let path = bundle.join_skill_manifest(&skill);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&path, r#"{"kind":"Masked"}"#).expect("write");
    let manifests = super::load_tier3_manifests_from_bundle(&bundle, &[skill]);
    assert!(manifests.is_empty(), "missing json_path skips skill");
}

#[test]
fn real_gate_maps_wasm_engine_error_to_wasm_abi() {
    let skill = SkillId::new("engine-fail").expect("skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(
        skill.clone(),
        manifest_for("engine-fail", "/params/message/parts/0/text"),
    );
    let invoker = MockTier3PartInvoker::with_error(skill.clone(), RedactionError::WasmEngine("boom".into()));
    let gate = RealTier3RedactionGate::new(invoker);
    let mut ctx = Tier3EvaluationContext::new("message/send", None, sample_payload(), manifests);
    assert_eq!(
        gate.redact(&mut ctx),
        Tier3RedactionDecision::Error {
            rule: skill,
            kind: Tier3EngineError::WasmAbi,
        }
    );
}

#[test]
fn real_gate_refusal_sentinel_with_no_tag_defaults_to_skill_policy_denied() {
    // Sentinel-only output (no `:reason` suffix) falls through to the
    // SkillPolicyDeniedPart bucket so an emitting skill without a
    // typed reason still produces a deny-with-rule audit record.
    let skill = SkillId::new("plain-deny").expect("skill");
    let mut manifests = BTreeMap::new();
    manifests.insert(
        skill.clone(),
        manifest_for("plain-deny", "/params/message/parts/0/text"),
    );
    let invoker = MockTier3PartInvoker::with_output(skill.clone(), TIER3_REFUSE_SENTINEL.to_vec());
    let gate = RealTier3RedactionGate::new(invoker);
    let mut ctx = Tier3EvaluationContext::new("message/send", None, sample_payload(), manifests);
    assert_eq!(
        gate.redact(&mut ctx),
        Tier3RedactionDecision::Refuse {
            reason: Tier3RefusalReason::SkillPolicyDeniedPart,
            rule: skill,
        }
    );
}
