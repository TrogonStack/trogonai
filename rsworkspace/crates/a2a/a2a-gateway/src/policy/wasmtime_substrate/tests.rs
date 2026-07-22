use std::path::PathBuf;

use a2a::types::{Artifact, PartContent, Role};
use a2a_nats::A2aMethod;
use a2a_redaction::{RedactionError, SkillId, WasmBundlePath};

use super::*;
use crate::policy::tier2::{NoopTier2Evaluator, Tier2Decision, Tier2EvaluationContext};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../a2a-redaction/tests/fixtures/identity_redact_part.wasm")
}

#[test]
fn register_redaction_skill_round_trips_fixture() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let substrate = WasmtimeSubstrate::try_new(dir).expect("wasmtime substrate");
    let wasm = std::fs::read(fixture_path()).expect("read identity fixture");
    let skill = SkillId::new("fixture").expect("non-empty test skill");
    substrate
        .register_redaction_skill(skill.clone(), &wasm)
        .expect("register wasm");

    let msg_in = Message {
        message_id: "m".into(),
        context_id: None,
        task_id: None,
        role: Role::Agent,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("x".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let got = substrate
        .redact_message_parts(msg_in.clone(), &skill)
        .expect("redact message");
    assert_eq!(
        serde_json::to_value(&got).expect("serialize got"),
        serde_json::to_value(&msg_in).expect("serialize in"),
    );

    let art_in = Artifact {
        artifact_id: "a".into(),
        name: None,
        description: None,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("blob".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
    };
    let got_art = substrate
        .redact_artifact_parts(art_in.clone(), &skill)
        .expect("redact artifact");
    assert_eq!(
        serde_json::to_value(&got_art).expect("serialize got"),
        serde_json::to_value(&art_in).expect("serialize in"),
    );
}

#[test]
fn preload_redaction_skill_surfaces_missing_bundle_as_policy_error() {
    // The substrate maps `RedactionError::WasmModule` (filesystem
    // read failure on the bundle) into `PolicyError::Redaction`.
    // Assert the typed variant so a future refactor that loses the
    // `?`-propagation or swaps the error mapping fails this test
    // instead of silently morphing the audit shape.
    let dir = tempfile::tempdir().expect("tempdir");
    let substrate = WasmtimeSubstrate::try_new(WasmBundlePath::new(dir.path())).expect("wasmtime substrate");
    let err = substrate
        .preload_redaction_skill(SkillId::new("does-not-exist").expect("non-empty"))
        .expect_err("missing bundle must error");
    assert!(
        matches!(err, PolicyError::Redaction(RedactionError::WasmModule(_))),
        "expected PolicyError::Redaction(WasmModule), got {err:?}",
    );
}

#[test]
fn noop_tier2_evaluator_returns_allow() {
    let evaluator = NoopTier2Evaluator;
    let ctx = Tier2EvaluationContext::new(
        A2aMethod::MessageSend,
        serde_json::json!({}),
        None,
        a2a_nats::A2aAgentId::new("planner").expect("nats-safe test agent"),
        None,
        Default::default(),
    );
    assert_eq!(evaluator.evaluate(&ctx), Tier2Decision::Allow);
}

#[test]
fn try_new_defaults_tier2_to_inactive() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let substrate = WasmtimeSubstrate::try_new(dir).expect("wasmtime substrate");
    assert!(
        !substrate.tier2.is_active(),
        "default Tier-2 wiring must start inactive so the cel engine isn't paid for unless explicitly enabled",
    );
    assert!(
        substrate.tier2.evaluator().is_none(),
        "an inactive Tier-2 state must not expose an evaluator — callers should branch on the typed state",
    );
}

#[test]
fn try_new_with_tier2_active_exposes_evaluator() {
    // Active Tier-2 carries its evaluator inside the variant — the
    // typed state is the single source of truth for both "is CEL
    // turned on?" and "which evaluator runs".
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let substrate = WasmtimeSubstrate::try_new_with_tier2(dir, Tier2State::Active(Box::new(NoopTier2Evaluator)), None)
        .expect("wasmtime substrate");
    assert!(substrate.tier2.is_active());
    assert!(
        substrate.tier2.evaluator().is_some(),
        "Tier2State::Active must surface the contained evaluator",
    );
}

#[test]
fn tier2_state_inactive_yields_no_evaluator() {
    // Construction of `Tier2State::Inactive` can't smuggle in a
    // dangling evaluator — there's no field to hold one. Pin the
    // shape so a future variant addition surfaces here.
    let state = Tier2State::Inactive;
    assert!(!state.is_active());
    assert!(state.evaluator().is_none());
}
