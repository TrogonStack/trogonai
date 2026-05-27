use std::path::Path;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::DecodePrivateKey;

use super::{ManifestSigner, SignerError, sign_payload_with_key};

const DEFAULT_KEY_ID: &str = "registry-signer-current";

pub struct FileManifestSigner {
    key_id: String,
    signing_key: SigningKey,
}

impl FileManifestSigner {
    pub fn from_pem_path(path: &Path) -> Result<Self, SignerError> {
        let pem = std::fs::read_to_string(path).map_err(SignerError::Io)?;
        Self::from_pem(&pem)
    }

    pub fn from_pem(pem: &str) -> Result<Self, SignerError> {
        let signing_key = SigningKey::from_pkcs8_pem(pem)
            .map_err(|error| SignerError::Parse(format!("invalid Ed25519 PKCS8 PEM: {error}")))?;
        Ok(Self {
            key_id: DEFAULT_KEY_ID.to_string(),
            signing_key,
        })
    }

    pub fn with_key_id(mut self, key_id: impl Into<String>) -> Self {
        self.key_id = key_id.into();
        self
    }
}

#[async_trait]
impl ManifestSigner for FileManifestSigner {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signing_key.verifying_key()
    }

    fn sign_payload(&self, payload: &[u8]) -> Result<crate::manifest::ManifestSignature, SignerError> {
        sign_payload_with_key(payload, &self.signing_key, &self.key_id)
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;

    use crate::manifest::{ManifestRecord, signing_payload, verify_manifest_signature};

    use super::*;

    #[test]
    fn file_signer_round_trip() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let pem = signing_key.to_pkcs8_pem(LineEnding::LF).expect("pem");
        let signer = FileManifestSigner::from_pem(&pem).expect("load signer");
        let record = ManifestRecord {
            agent_id: "acme/bot".into(),
            agent_version: "1.0.0".into(),
            agent_definition_digest: "sha256:deadbeef".into(),
            owner_team: "platform".into(),
            allowed_workloads: vec![],
            allowed_tools: vec![],
            allowed_audiences: vec![],
            allowed_purposes: vec![],
            mesh_token_ttl_s: None,
            metadata: serde_json::Value::Null,
            lifecycle_state: trogon_agent_registry::LifecycleState::Active,
        };
        let payload = signing_payload(&record).expect("payload");
        let signature = signer.sign_payload(&payload).expect("sign");
        verify_manifest_signature(&record, &signature, &signer.verifying_key()).expect("verify");
    }
}
