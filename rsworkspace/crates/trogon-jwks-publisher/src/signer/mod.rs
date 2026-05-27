use std::collections::HashMap;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::Header;
use serde_json::Value;

use crate::jwks::Jwks;
use crate::keys::KeyError;

pub mod file;

#[cfg(feature = "kms-aws")]
pub mod kms;

#[cfg(feature = "vault")]
pub mod vault;

pub const DEFAULT_MESH_ISSUER: &str = "urn:trogon:sts:mesh";
pub const MAX_MESH_TOKEN_TTL_SECS: u64 = 300;
pub const DEFAULT_JWT_LEEWAY_SECS: u64 = 60;
pub const MIN_ROTATION_OVERLAP_SECS: u64 = MAX_MESH_TOKEN_TTL_SECS + DEFAULT_JWT_LEEWAY_SECS;

#[async_trait]
pub trait MeshSigner: Send + Sync {
    fn current_kid(&self) -> &str;
    fn mesh_issuer(&self) -> &str;
    async fn sign(&self, claims: &HashMap<String, Value>) -> Result<String, KeyError>;
    async fn active_jwks(&self) -> Result<Jwks, KeyError>;
}

pub fn assemble_rs256_jwt(header: &Header, claims: &HashMap<String, Value>, signature: &[u8]) -> Result<String, KeyError> {
    let header_json = serde_json::to_vec(header).map_err(|e| KeyError::Sign(format!("jwt header: {e}")))?;
    let claims_json = serde_json::to_vec(claims).map_err(|e| KeyError::Sign(format!("jwt claims: {e}")))?;
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header_json),
        URL_SAFE_NO_PAD.encode(claims_json)
    );
    Ok(format!(
        "{}.{}",
        signing_input,
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub fn rs256_signing_input(header: &Header, claims: &HashMap<String, Value>) -> Result<Vec<u8>, KeyError> {
    let header_json = serde_json::to_vec(header).map_err(|e| KeyError::Sign(format!("jwt header: {e}")))?;
    let claims_json = serde_json::to_vec(claims).map_err(|e| KeyError::Sign(format!("jwt claims: {e}")))?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header_json),
        URL_SAFE_NO_PAD.encode(claims_json)
    )
    .into_bytes())
}

pub fn mesh_claims(issuer: &str, subject: &str, ttl_secs: u64) -> HashMap<String, Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let mut claims = HashMap::new();
    claims.insert("iss".into(), Value::String(issuer.into()));
    claims.insert("sub".into(), Value::String(subject.into()));
    claims.insert("iat".into(), Value::Number(now.into()));
    claims.insert("exp".into(), Value::Number((now + ttl_secs as i64).into()));
    claims
}
