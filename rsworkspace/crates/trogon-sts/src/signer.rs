use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::Value;

use crate::error::StsError;

#[async_trait]
pub trait Signer: Send + Sync {
    fn current_kid(&self) -> String;
    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, StsError>;
}

pub struct FileSigner {
    kid: String,
    encoding_key: EncodingKey,
    algorithm: Algorithm,
}

impl FileSigner {
    pub fn from_rsa_pem(pem: &str, kid: impl Into<String>) -> Result<Self, StsError> {
        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|e| StsError::ServerError(format!("invalid signing key PEM: {e}")))?;
        Ok(Self {
            kid: kid.into(),
            encoding_key,
            algorithm: Algorithm::RS256,
        })
    }
}

#[async_trait]
impl Signer for FileSigner {
    fn current_kid(&self) -> String {
        self.kid.clone()
    }

    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, StsError> {
        let mut header = Header::new(self.algorithm);
        header.kid = Some(self.kid.clone());
        header.typ = Some("JWT".into());
        encode(&header, claims, &self.encoding_key).map_err(|e| StsError::ServerError(format!("sign mesh token: {e}")))
    }
}

/// TODO(prod): Cloud KMS asymmetric sign backend (ADR 0006).
pub struct KmsSigner;

impl Default for KmsSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl KmsSigner {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Signer for KmsSigner {
    fn current_kid(&self) -> String {
        todo!("KmsSigner::current_kid — wire AWS/GCP KMS key version id")
    }

    async fn sign(&self, _claims: &HashMap<String, Value>) -> Result<String, StsError> {
        todo!("KmsSigner::sign — call cloud KMS Sign API")
    }
}

/// TODO(prod): HashiCorp Vault Transit sign backend (ADR 0006).
pub struct VaultSigner;

impl Default for VaultSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl VaultSigner {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Signer for VaultSigner {
    fn current_kid(&self) -> String {
        todo!("VaultSigner::current_kid — map Vault key version to kid")
    }

    async fn sign(&self, _claims: &HashMap<String, Value>) -> Result<String, StsError> {
        todo!("VaultSigner::sign — call Vault transit sign endpoint")
    }
}

pub type DynSigner = Arc<dyn Signer>;
