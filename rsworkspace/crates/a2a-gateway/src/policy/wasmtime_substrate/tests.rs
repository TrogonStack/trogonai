use std::path::PathBuf;

use a2a::types::{Artifact, PartContent, Role};
use a2a_nats::A2aMethod;
use a2a_redaction::{SkillId, WasmBundlePath};

use super::*;
use crate::policy::tier2::{Tier2Decision, Tier2EvaluationContext};

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
fn try_new_defaults_tier2_to_noop_inactive() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let substrate = WasmtimeSubstrate::try_new(dir).expect("wasmtime substrate");
    assert!(
        !substrate.tier2_cel_active,
        "default Tier-2 wiring must start inactive so the cel engine isn't paid for unless explicitly enabled",
    );
}
