//! Wasmtime-backed substrate that hosts the Tier-2 CEL evaluator
//! and the Tier-3 Wasm redactor together.
//!
//! Bundling both behind one substrate keeps the gateway boot path
//! from spinning up two wasmtime engines per skill — the engine
//! warm-up is the slowest part of the Tier-3 critical section, so
//! co-locating CEL and redaction here also lets a future shared
//! engine optimization land in one place.

use a2a::types::{Artifact, Message};
use a2a_redaction::{Ed25519PublicKey, Redactor, SkillId, WasmBundlePath, wasm::WasmRedactorHost};

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

    pub fn register_redaction_skill(&self, skill: SkillId, wasm_binary: &[u8]) -> Result<(), PolicyError> {
        Ok(self.redaction.register_skill_wasm(skill, wasm_binary)?)
    }

    pub fn redact_message_parts(&self, message: Message, skill: &SkillId) -> Result<Message, PolicyError> {
        Ok(self.redaction.redact_message(message, skill)?)
    }

    pub fn redact_artifact_parts(&self, artifact: Artifact, skill: &SkillId) -> Result<Artifact, PolicyError> {
        Ok(self.redaction.redact_artifact(artifact, skill)?)
    }
}

#[cfg(test)]
mod tests;
