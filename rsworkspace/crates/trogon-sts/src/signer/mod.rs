use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use jsonwebtoken::{Algorithm, Header};
use serde_json::Value;

use crate::error::StsError;

mod file;
#[cfg(feature = "kms-aws")]
mod kms;
#[cfg(feature = "vault")]
mod vault;

pub use file::FileSigner;
#[cfg(feature = "kms-aws")]
pub use kms::KmsSigner;
#[cfg(feature = "vault")]
pub use vault::VaultSigner;

#[async_trait]
pub trait Signer: Send + Sync {
    fn current_kid(&self) -> String;
    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, StsError>;
}

pub type DynSigner = Arc<dyn Signer>;

pub(crate) fn jwt_signing_input(algorithm: Algorithm, kid: &str, claims: &HashMap<String, Value>) -> Result<String, StsError> {
    let mut header = Header::new(algorithm);
    header.kid = Some(kid.to_string());
    header.typ = Some("JWT".into());
    let header_json = serde_json::to_vec(&header)
        .map_err(|e| StsError::ServerError(format!("jwt header json: {e}")))?;
    let payload_json = serde_json::to_vec(claims)
        .map_err(|e| StsError::ServerError(format!("jwt payload json: {e}")))?;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    Ok(format!(
        "{}.{}",
        engine.encode(header_json),
        engine.encode(payload_json)
    ))
}

pub(crate) fn assemble_jwt(signing_input: &str, signature: &[u8]) -> String {
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!("{}.{}", signing_input, engine.encode(signature))
}
