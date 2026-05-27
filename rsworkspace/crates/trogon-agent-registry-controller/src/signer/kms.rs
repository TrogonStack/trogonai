use async_trait::async_trait;
use ed25519_dalek::VerifyingKey;

use super::{ManifestSigner, SignerError};

pub struct KmsManifestSigner;

impl KmsManifestSigner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KmsManifestSigner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ManifestSigner for KmsManifestSigner {
    fn key_id(&self) -> &str {
        "kms-stub"
    }

    fn verifying_key(&self) -> VerifyingKey {
        todo!("kms backend selection — tracked in carry-over §JWKS production custody")
    }

    fn sign_payload(&self, _payload: &[u8]) -> Result<crate::manifest::ManifestSignature, SignerError> {
        todo!("kms backend selection — tracked in carry-over §JWKS production custody")
    }
}
