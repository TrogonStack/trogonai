mod engine;

use std::collections::HashMap;
use std::sync::{Once, RwLock};

use a2a::types::{Artifact, Message};
use wasmtime::{Engine, Module};

use crate::error::RedactionError;
use crate::redactor::{self, Redactor};
use crate::signed_bundle::{Ed25519PublicKey, SignatureVerificationError, SignedBundleManifest, verify_signed_bundle};
use crate::skill_id::SkillId;
use crate::tier3_sentinel::{output_is_tier3_refusal, tier3_refusal_reason_tag};
use crate::wasm_bundle_path::WasmBundlePath;

/// Run the guest, then surface the documented Tier-3 refusal sentinel as a
/// typed `RedactionError::Tier3Refusal` instead of letting it fall through
/// the JSON-decode path where it would surface as a generic `Json` error.
fn redact_part_or_tier3_refusal(
    engine: &wasmtime::Engine,
    wasm_mod: &Module,
    payload: &[u8],
) -> Result<Vec<u8>, RedactionError> {
    let output = engine::redact_part_guest(engine, wasm_mod, payload)?;
    if output_is_tier3_refusal(&output) {
        return Err(RedactionError::Tier3Refusal(
            tier3_refusal_reason_tag(&output).map(str::to_owned),
        ));
    }
    Ok(output)
}

static SIGNING_DISABLED_WARN: Once = Once::new();

pub struct WasmRedactorHost {
    engine: Engine,
    modules: RwLock<HashMap<SkillId, Module>>,
    bundles_base: WasmBundlePath,
    signing_pubkey: Option<Ed25519PublicKey>,
}

impl WasmRedactorHost {
    pub fn new(bundles_base: WasmBundlePath) -> Result<Self, RedactionError> {
        Self::new_with_signing_pubkey(bundles_base, None)
    }

    pub fn new_with_signing_pubkey(
        bundles_base: WasmBundlePath,
        signing_pubkey: Option<Ed25519PublicKey>,
    ) -> Result<Self, RedactionError> {
        if signing_pubkey.is_none() {
            SIGNING_DISABLED_WARN.call_once(|| {
                tracing::warn!("A2A_GATEWAY_TIER3_SIGNING_PUBKEY unset; tier-3 bundle signature verification disabled");
            });
        }

        Ok(Self {
            engine: engine::new_engine()?,
            modules: RwLock::new(HashMap::new()),
            bundles_base,
            signing_pubkey,
        })
    }

    pub fn bundles_base(&self) -> &WasmBundlePath {
        &self.bundles_base
    }

    pub fn signing_pubkey(&self) -> Option<&Ed25519PublicKey> {
        self.signing_pubkey.as_ref()
    }

    /// Register a wasm module that bypasses signature verification.
    ///
    /// Refuses when the host was constructed with a signing public key —
    /// otherwise a caller could side-step the configured trust boundary by
    /// dropping arbitrary bytes into the cache (and overwrite a previously
    /// verified module for the same `SkillId`). The signed entry point is
    /// `preload_skill_bundle`. Test/fixture callers that don't configure a
    /// signing pubkey can still use this directly.
    pub fn register_skill_wasm(&self, skill: SkillId, wasm_binary: &[u8]) -> Result<(), RedactionError> {
        if self.signing_pubkey.is_some() {
            return Err(RedactionError::Signature(
                SignatureVerificationError::MissingSignatureFile {
                    skill_id: skill.to_string(),
                    path: "register_skill_wasm bypassed signature verification".into(),
                },
            ));
        }
        self.register_skill_wasm_unchecked(skill, wasm_binary)
    }

    fn register_skill_wasm_unchecked(&self, skill: SkillId, wasm_binary: &[u8]) -> Result<(), RedactionError> {
        let compiled = Module::from_binary(&self.engine, wasm_binary)
            .map_err(|e| RedactionError::WasmModule(format!("skill {skill}: {e}")))?;
        let mut guard = self.modules.write().unwrap_or_else(|e| e.into_inner());
        guard.insert(skill, compiled);
        Ok(())
    }

    pub fn preload_skill_bundle(&self, skill: SkillId) -> Result<(), RedactionError> {
        let wasm_path = self.bundles_base.join_skill_wasm(&skill);
        let manifest_path = self.bundles_base.join_skill_manifest(&skill);
        let wasm_bytes = std::fs::read(&wasm_path)
            .map_err(|err| RedactionError::WasmModule(format!("read {}: {err}", wasm_path.display())))?;
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|err| {
            RedactionError::WasmModule(format!(
                "skill {skill}: read manifest {}: {err}",
                manifest_path.display()
            ))
        })?;

        if let Some(pubkey) = self.signing_pubkey.as_ref() {
            let sig_path = self.bundles_base.join_skill_sig(&skill);
            let sig_bytes = std::fs::read(&sig_path).map_err(|_| SignatureVerificationError::MissingSignatureFile {
                skill_id: skill.to_string(),
                path: sig_path.display().to_string(),
            })?;
            let envelope = SignedBundleManifest::parse_json(&sig_bytes, &skill)?;
            verify_signed_bundle(pubkey, &manifest_bytes, &wasm_bytes, &envelope)?;
        }

        self.register_skill_wasm_unchecked(skill, &wasm_bytes)
    }

    pub fn register_skill_bundle_file(&self, skill: SkillId) -> Result<(), RedactionError> {
        self.preload_skill_bundle(skill)
    }

    pub fn redact_part_bytes(&self, skill: &SkillId, payload: &[u8]) -> Result<Vec<u8>, RedactionError> {
        let modules = self.modules.read().unwrap_or_else(|e| {
            tracing::error!("wasm redactor skill module cache poisoned after write failure");
            e.into_inner()
        });

        let Some(wasm_mod) = modules.get(skill) else {
            return Ok(payload.to_vec());
        };

        redact_part_or_tier3_refusal(&self.engine, wasm_mod, payload)
    }
}

impl Redactor for WasmRedactorHost {
    fn redact_message(&self, message: Message, skill: &SkillId) -> Result<Message, RedactionError> {
        let modules = self.modules.read().unwrap_or_else(|e| {
            tracing::error!("wasm redactor skill module cache poisoned after write failure");
            e.into_inner()
        });

        let Some(wasm_mod) = modules.get(skill) else {
            return Ok(message);
        };

        redactor::redact_message_parts_with(message, |json| {
            redact_part_or_tier3_refusal(&self.engine, wasm_mod, json)
        })
    }

    fn redact_artifact(&self, artifact: Artifact, skill: &SkillId) -> Result<Artifact, RedactionError> {
        let modules = self.modules.read().unwrap_or_else(|e| {
            tracing::error!("wasm redactor skill module cache poisoned after write failure");
            e.into_inner()
        });

        let Some(wasm_mod) = modules.get(skill) else {
            return Ok(artifact);
        };

        redactor::redact_artifact_parts_with(artifact, |json| {
            redact_part_or_tier3_refusal(&self.engine, wasm_mod, json)
        })
    }
}

#[cfg(test)]
mod tests;
