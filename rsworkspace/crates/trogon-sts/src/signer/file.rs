use std::collections::HashMap;

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::Value;

use super::Signer;
use crate::error::StsError;

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
        encode(&header, claims, &self.encoding_key)
            .map_err(|e| StsError::SignerUnavailable(format!("sign mesh token: {e}")))
    }
}
