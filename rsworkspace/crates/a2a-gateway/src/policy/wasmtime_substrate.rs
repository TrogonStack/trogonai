use a2a_redaction::{
    wasm::WasmRedactorHost,
    Ed25519PublicKey,
    Redactor,
    SkillId,
    WasmBundlePath,
};
use a2a_types::{Artifact, Message};

use crate::policy::error::PolicyError;
use crate::policy::tier2::{NoopTier2Evaluator, Tier2CelEvaluator};

pub struct WasmtimeSubstrate {
    pub redaction: WasmRedactorHost,
    pub tier2: Box<dyn Tier2CelEvaluator>,
    pub tier2_cel_active: bool,
}

impl WasmtimeSubstrate {
    pub fn try_new_with_tier2(
        bundles_base: WasmBundlePath,
        tier2: Box<dyn Tier2CelEvaluator>,
        tier2_cel_active: bool,
        signing_pubkey: Option<Ed25519PublicKey>,
    ) -> Result<Self, PolicyError> {
        let host = WasmRedactorHost::new_with_signing_pubkey(bundles_base, signing_pubkey)?;
        Ok(Self {
            redaction: host,
            tier2,
            tier2_cel_active,
        })
    }

    pub fn try_new(bundles_base: WasmBundlePath) -> Result<Self, PolicyError> {
        Self::try_new_with_tier2(bundles_base, Box::new(NoopTier2Evaluator), false, None)
    }

    pub fn preload_redaction_skill(&self, skill: SkillId) -> Result<(), PolicyError> {
        Ok(self.redaction.preload_skill_bundle(skill)?)
    }

    pub fn register_redaction_skill(
        &self,
        skill: SkillId,
        wasm_binary: &[u8],
    ) -> Result<(), PolicyError> {
        Ok(self.redaction.register_skill_wasm(skill, wasm_binary)?)
    }

    pub fn redact_message_parts(
        &self,
        message: Message,
        skill: &SkillId,
    ) -> Result<Message, PolicyError> {
        Ok(self.redaction.redact_message(message, skill)?)
    }

    pub fn redact_artifact_parts(
        &self,
        artifact: Artifact,
        skill: &SkillId,
    ) -> Result<Artifact, PolicyError> {
        Ok(self.redaction.redact_artifact(artifact, skill)?)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use a2a_redaction::{SkillId, WasmBundlePath};
    use a2a_types::part;
    use a2a_types::{Artifact, Role};
    use crate::policy::tier2::{Tier2Decision, Tier2EvaluationContext};

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../a2a-redaction/tests/fixtures/identity_redact_part.wasm")
    }

    #[test]
    fn register_redaction_skill_round_trips_fixture() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let substrate = WasmtimeSubstrate::try_new(dir).expect("wasmtime substrate");
        let wasm = std::fs::read(fixture_path()).expect("read identity fixture");
        let skill = SkillId::new("fixture");
        substrate.register_redaction_skill(skill.clone(), &wasm).expect("register wasm");

        let msg_in = Message {
            message_id: "m".into(),
            role: Role::Agent.into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("x".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = substrate.redact_message_parts(msg_in.clone(), &skill).expect("redact message");
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            serde_json::to_value(&msg_in).unwrap()
        );

        let art_in = Artifact {
            artifact_id: "a".into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("blob".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got_art = substrate
            .redact_artifact_parts(art_in.clone(), &skill)
            .expect("redact artifact");
        assert_eq!(
            serde_json::to_value(&got_art).unwrap(),
            serde_json::to_value(&art_in).unwrap()
        );
    }

    #[test]
    fn noop_tier2_evaluator_returns_allow() {
        let evaluator = NoopTier2Evaluator;
        let ctx = Tier2EvaluationContext::new(
            "message/send",
            serde_json::json!({}),
            None,
            a2a_nats::A2aAgentId::new("planner").unwrap(),
            None,
            Default::default(),
        );
        assert_eq!(evaluator.evaluate(&ctx), Tier2Decision::Allow);
    }
}
