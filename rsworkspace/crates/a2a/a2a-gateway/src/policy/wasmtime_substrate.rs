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
use crate::policy::tier2::Tier2CelEvaluator;

/// Tier-2 wiring state. The active/evaluator pair used to live as
/// two independent fields, which let callers represent invalid
/// combinations (`Noop` + `active=true` or a real evaluator +
/// `active=false`). Folding both into one enum makes the contradictory
/// states unrepresentable: `Inactive` means the gateway never even
/// asks for an evaluation; `Active(_)` means the supplied evaluator
/// is the one that runs.
pub enum Tier2State {
    Inactive,
    Active(Box<dyn Tier2CelEvaluator>),
}

impl Tier2State {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active(_))
    }

    /// Return the active evaluator, or `None` when Tier-2 is
    /// inactive. Callers that need the evaluator must handle the
    /// `None` arm explicitly so an inactive state can't be silently
    /// papered over by a Noop default.
    pub fn evaluator(&self) -> Option<&dyn Tier2CelEvaluator> {
        match self {
            Self::Active(eval) => Some(eval.as_ref()),
            Self::Inactive => None,
        }
    }
}

pub struct WasmtimeSubstrate {
    pub redaction: WasmRedactorHost,
    pub tier2: Tier2State,
}

impl WasmtimeSubstrate {
    pub fn try_new_with_tier2(
        bundles_base: WasmBundlePath,
        tier2: Tier2State,
        signing_pubkey: Option<Ed25519PublicKey>,
    ) -> Result<Self, PolicyError> {
        let host = WasmRedactorHost::new_with_signing_pubkey(bundles_base, signing_pubkey)?;
        Ok(Self { redaction: host, tier2 })
    }

    pub fn try_new(bundles_base: WasmBundlePath) -> Result<Self, PolicyError> {
        Self::try_new_with_tier2(bundles_base, Tier2State::Inactive, None)
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
