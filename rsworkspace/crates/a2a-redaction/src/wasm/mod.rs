mod engine;

use std::collections::HashMap;
use std::sync::{Once, RwLock};

use a2a_types::{Artifact, Message};
use wasmtime::{Engine, Module};

use crate::error::RedactionError;
use crate::redactor::{self, Redactor};
use crate::signed_bundle::{Ed25519PublicKey, SignedBundleManifest, verify_signed_bundle, SignatureVerificationError};
use crate::skill_id::SkillId;
use crate::wasm_bundle_path::WasmBundlePath;

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
                tracing::warn!(
                    "A2A_GATEWAY_TIER3_SIGNING_PUBKEY unset; tier-3 bundle signature verification disabled"
                );
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

    pub fn register_skill_wasm(&self, skill: SkillId, wasm_binary: &[u8]) -> Result<(), RedactionError> {
        let compiled = Module::from_binary(&self.engine, wasm_binary)
            .map_err(|e| RedactionError::WasmModule(format!("skill {skill}: {e}")))?;
        let mut guard = self
            .modules
            .write()
            .unwrap_or_else(|e| e.into_inner());
        guard.insert(skill, compiled);
        Ok(())
    }

    pub fn preload_skill_bundle(&self, skill: SkillId) -> Result<(), RedactionError> {
        let wasm_path = self.bundles_base.join_skill_wasm(&skill);
        let manifest_path = self.bundles_base.join_skill_manifest(&skill);
        let wasm_bytes = std::fs::read(&wasm_path).map_err(|err| {
            RedactionError::WasmModule(format!("read {}: {err}", wasm_path.display()))
        })?;
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|err| {
            RedactionError::WasmModule(format!(
                "skill {skill}: read manifest {}: {err}",
                manifest_path.display()
            ))
        })?;

        if let Some(pubkey) = self.signing_pubkey.as_ref() {
            let sig_path = self.bundles_base.join_skill_sig(&skill);
            let sig_bytes = std::fs::read(&sig_path).map_err(|err| {
                RedactionError::WasmModule(format!(
                    "skill {skill}: {}",
                    SignatureVerificationError::MissingSignatureFile {
                        skill_id: skill.to_string(),
                        path: format!("{}: {err}", sig_path.display()),
                    }
                ))
            })?;
            let envelope = SignedBundleManifest::parse_json(&sig_bytes, &skill)
                .map_err(|err| RedactionError::WasmModule(format!("skill {skill}: {err}")))?;
            verify_signed_bundle(pubkey, &manifest_bytes, &wasm_bytes, &envelope)
                .map_err(|err| RedactionError::WasmModule(format!("skill {skill}: {err}")))?;
        }

        self.register_skill_wasm(skill, &wasm_bytes)
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

        engine::redact_part_guest(&self.engine, wasm_mod, payload)
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

        redactor::redact_message_parts_with(message, |json| engine::redact_part_guest(&self.engine, wasm_mod, json))
    }

    fn redact_artifact(&self, artifact: Artifact, skill: &SkillId) -> Result<Artifact, RedactionError> {
        let modules = self.modules.read().unwrap_or_else(|e| {
            tracing::error!("wasm redactor skill module cache poisoned after write failure");
            e.into_inner()
        });

        let Some(wasm_mod) = modules.get(skill) else {
            return Ok(artifact);
        };

        redactor::redact_artifact_parts_with(artifact, |json| engine::redact_part_guest(&self.engine, wasm_mod, json))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::signed_bundle::{Ed25519Signature, Sha256Digest, sign_bundle_digest};
    use a2a_types::{part, Role};

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
        let manifest_digest = Sha256Digest::hash(manifest_bytes);
        let wasm_digest = Sha256Digest::hash(wasm_bytes);
        let message = sign_bundle_digest(manifest_digest, wasm_digest);
        let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
        let envelope = SignedBundleManifest::new(
            &SkillId::new(skill),
            manifest_digest,
            wasm_digest,
            signature,
        );
        let sig_json = serde_json::to_vec_pretty(&envelope).expect("serialize sig");
        fs::write(dir.join(format!("{skill}.sig")), sig_json).expect("write sig");
    }

    #[test]
    fn passthrough_when_no_registered_module_for_message() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let msg = Message {
            message_id: "m".into(),
            role: Role::Agent.into(),
            ..Default::default()
        };
        let out = host.redact_message(msg.clone(), &SkillId::new("missing")).unwrap();
        assert_eq!(
            serde_json::to_value(out).unwrap(),
            serde_json::to_value(msg).unwrap()
        );
    }

    #[test]
    fn passthrough_when_no_registered_module_for_artifact() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let art = Artifact {
            artifact_id: "aid".into(),
            ..Default::default()
        };
        let out = host.redact_artifact(art.clone(), &SkillId::new("missing")).unwrap();
        assert_eq!(
            serde_json::to_value(out).unwrap(),
            serde_json::to_value(art).unwrap()
        );
    }

    #[test]
    fn wasm_skill_dispatches_through_message_parts() {
        let dir = WasmBundlePath::new(std::env::temp_dir());
        let host = WasmRedactorHost::new(dir).unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let skill = SkillId::new("fixture");
        host.register_skill_wasm(skill.clone(), wasm).unwrap();

        let msg_in = Message {
            message_id: "m".into(),
            role: Role::Agent.into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("x".into())),
                ..Default::default()
            }],
            ..Default::default()
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
        let skill = SkillId::new("fixture");
        host.register_skill_wasm(skill.clone(), wasm).unwrap();

        let art_in = Artifact {
            artifact_id: "a".into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("blob".into())),
                ..Default::default()
            }],
            ..Default::default()
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
        host.preload_skill_bundle(SkillId::new(skill)).expect("preload");
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
        host.preload_skill_bundle(SkillId::new(skill)).expect("preload");
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
            .preload_skill_bundle(SkillId::new(skill))
            .expect_err("missing sig");
        assert!(matches!(err, RedactionError::WasmModule(_)));
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
            .preload_skill_bundle(SkillId::new(skill))
            .expect_err("tampered wasm");
        assert!(matches!(err, RedactionError::WasmModule(_)));
    }
}
