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
mod tests {
    use std::fs;
    use std::path::Path;

    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::signed_bundle::{Ed25519Signature, Sha256Digest, sign_bundle_digest};
    use a2a::types::{PartContent, Role};

    fn fixture_path() -> &'static Path {
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ))
    }

    fn write_signed_bundle(
        dir: &Path,
        skill: &str,
        signing_key: &SigningKey,
        manifest_bytes: &[u8],
        wasm_bytes: &[u8],
    ) {
        fs::write(dir.join(format!("{skill}.wasm")), wasm_bytes).expect("write wasm");
        fs::write(dir.join(format!("{skill}.manifest.json")), manifest_bytes).expect("write manifest");
        let sid = SkillId::new(skill).expect("valid");
        let manifest_digest = Sha256Digest::hash(manifest_bytes);
        let wasm_digest = Sha256Digest::hash(wasm_bytes);
        let message = sign_bundle_digest(
            crate::signed_bundle::SIGNED_BUNDLE_VERSION,
            &sid,
            manifest_digest,
            wasm_digest,
        );
        let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
        let envelope = SignedBundleManifest::new(&sid, manifest_digest, wasm_digest, signature);
        let sig_json = serde_json::to_vec_pretty(&envelope).expect("serialize sig");
        fs::write(dir.join(format!("{skill}.sig")), sig_json).expect("write sig");
    }

    #[test]
    fn passthrough_when_no_registered_module_for_message() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let msg = Message {
            message_id: "m".into(),
            context_id: None,
            task_id: None,
            role: Role::Agent,
            parts: vec![],
            metadata: None,
            extensions: None,
            reference_task_ids: None,
        };
        let out = host
            .redact_message(msg.clone(), &SkillId::new("missing").expect("valid"))
            .unwrap();
        assert_eq!(serde_json::to_value(out).unwrap(), serde_json::to_value(msg).unwrap());
    }

    #[test]
    fn passthrough_when_no_registered_module_for_artifact() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let art = Artifact {
            artifact_id: "aid".into(),
            name: None,
            description: None,
            parts: vec![],
            metadata: None,
            extensions: None,
        };
        let out = host
            .redact_artifact(art.clone(), &SkillId::new("missing").expect("valid"))
            .unwrap();
        assert_eq!(serde_json::to_value(out).unwrap(), serde_json::to_value(art).unwrap());
    }

    #[test]
    fn wasm_skill_dispatches_through_message_parts() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let skill = SkillId::new("fixture").expect("valid");
        host.register_skill_wasm(skill.clone(), wasm).unwrap();

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
        let got = host.redact_message(msg_in.clone(), &skill).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::to_value(msg_in).unwrap()
        );
    }

    #[test]
    fn wasm_skill_dispatches_through_artifact_parts() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let skill = SkillId::new("fixture").expect("valid");
        host.register_skill_wasm(skill.clone(), wasm).unwrap();

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
        let got = host.redact_artifact(art_in.clone(), &skill).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::to_value(art_in).unwrap()
        );
    }

    #[test]
    fn preload_without_signing_pubkey_skips_sig_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill = "fixture";
        let manifest = br#"{"json_path":"$.x"}"#;
        let wasm = fs::read(fixture_path()).expect("read fixture");
        fs::write(temp.path().join(format!("{skill}.wasm")), &wasm).expect("write wasm");
        fs::write(temp.path().join(format!("{skill}.manifest.json")), manifest).expect("write manifest");

        let host = WasmRedactorHost::new(WasmBundlePath::new(temp.path())).expect("host");
        host.preload_skill_bundle(SkillId::new(skill).expect("valid"))
            .expect("preload");
    }

    #[test]
    fn preload_with_signing_pubkey_requires_valid_sig() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill = "fixture";
        let manifest = br#"{"json_path":"$.x"}"#;
        let wasm = fs::read(fixture_path()).expect("read fixture");
        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
        write_signed_bundle(temp.path(), skill, &signing_key, manifest, &wasm);

        let host =
            WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
        host.preload_skill_bundle(SkillId::new(skill).expect("valid"))
            .expect("preload");
    }

    #[test]
    fn preload_with_signing_pubkey_rejects_missing_sig() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill = "fixture";
        let manifest = br#"{"json_path":"$.x"}"#;
        let wasm = fs::read(fixture_path()).expect("read fixture");
        fs::write(temp.path().join(format!("{skill}.wasm")), &wasm).expect("write wasm");
        fs::write(temp.path().join(format!("{skill}.manifest.json")), manifest).expect("write manifest");

        let signing_key = SigningKey::from_bytes(&[11u8; 32]);
        let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
        let host =
            WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
        let err = host
            .preload_skill_bundle(SkillId::new(skill).expect("valid"))
            .expect_err("missing sig");
        assert!(matches!(
            err,
            RedactionError::Signature(SignatureVerificationError::MissingSignatureFile { .. })
        ));
    }

    #[test]
    fn preload_with_signing_pubkey_rejects_tampered_wasm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill = "fixture";
        let manifest = br#"{"json_path":"$.x"}"#;
        let wasm = fs::read(fixture_path()).expect("read fixture");
        let signing_key = SigningKey::from_bytes(&[13u8; 32]);
        write_signed_bundle(temp.path(), skill, &signing_key, manifest, &wasm);

        let wasm_path = temp.path().join(format!("{skill}.wasm"));
        fs::write(&wasm_path, b"tampered").expect("tamper wasm");

        let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
        let host =
            WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
        let err = host
            .preload_skill_bundle(SkillId::new(skill).expect("valid"))
            .expect_err("tampered wasm");
        assert!(matches!(
            err,
            RedactionError::Signature(SignatureVerificationError::ManifestSha256Mismatch { .. })
                | RedactionError::Signature(SignatureVerificationError::WasmSha256Mismatch { .. })
                | RedactionError::Signature(SignatureVerificationError::SignatureVerificationFailed { .. })
        ));
    }

    #[test]
    fn tier3_refusal_error_display_renders_reason_tag() {
        // Pure rendering check for the new error variant. The wrapper's
        // sentinel-detection behavior is covered by the existing wasm
        // dispatch tests against the identity fixture, which the real
        // Tier-3 skills' integration tests exercise once their fixtures
        // land.
        let err = RedactionError::Tier3Refusal(Some("UnauthorizedDataCategory".into()));
        let rendered = err.to_string();
        assert!(rendered.contains("tier-3 skill refused"));
        assert!(rendered.contains("UnauthorizedDataCategory"));
    }

    #[test]
    fn register_skill_wasm_refused_when_signing_pubkey_configured() {
        let temp = tempfile::tempdir().expect("tempdir");
        let signing_key = SigningKey::from_bytes(&[19u8; 32]);
        let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
        let host =
            WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
        let wasm = fs::read(fixture_path()).expect("read fixture");
        let err = host
            .register_skill_wasm(SkillId::new("anything").expect("valid"), &wasm)
            .expect_err("must refuse bypass when signing pubkey configured");
        assert!(matches!(err, RedactionError::Signature(_)));
    }
}
