//! RFC 7638 JWK Thumbprint (SHA-256). Used to bind tokens to a specific key.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Compute the RFC 7638 SHA-256 thumbprint of a JWK.
///
/// Per RFC 7638, the canonical form is a JSON object with the JWK's "required
/// members" for the key type, sorted lexicographically by key name with no
/// extra whitespace. Building the JSON via `serde_json` keeps the string
/// values properly escaped — interpolating raw `format!` strings would
/// produce a non-canonical digest for values containing quotes or
/// backslashes, weakening the `agent_jkt` binding for crafted `cnf.jwk`
/// payloads.
///
/// Supports EC (P-256/P-384), OKP (Ed25519), RSA, and oct key types.
pub fn jwk_thumbprint(jwk: &serde_json::Value) -> Result<String, JktError> {
    let kty = jwk.get("kty").and_then(|v| v.as_str()).ok_or(JktError::MissingKty)?;
    let mut required = Map::new();
    match kty {
        "EC" => {
            require_str(jwk, "crv", &mut required)?;
            required.insert("kty".into(), Value::String("EC".into()));
            require_str(jwk, "x", &mut required)?;
            require_str(jwk, "y", &mut required)?;
        }
        "OKP" => {
            require_str(jwk, "crv", &mut required)?;
            required.insert("kty".into(), Value::String("OKP".into()));
            require_str(jwk, "x", &mut required)?;
        }
        "RSA" => {
            require_str(jwk, "e", &mut required)?;
            required.insert("kty".into(), Value::String("RSA".into()));
            require_str(jwk, "n", &mut required)?;
        }
        "oct" => {
            require_str(jwk, "k", &mut required)?;
            required.insert("kty".into(), Value::String("oct".into()));
        }
        other => return Err(JktError::UnsupportedKty(other.to_string())),
    }
    // serde_json::Map (with the `preserve_order` feature off) is a BTreeMap,
    // which already iterates lexicographically. Render with the default
    // compact serializer — RFC 7638 requires no whitespace between members.
    let canonical = serde_json::to_string(&Value::Object(required)).map_err(JktError::Serialize)?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(digest))
}

fn require_str(jwk: &Value, field: &'static str, out: &mut Map<String, Value>) -> Result<(), JktError> {
    let value = jwk
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or(JktError::MissingField(field))?;
    out.insert(field.to_owned(), Value::String(value.to_owned()));
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum JktError {
    #[error("jwk missing required field: kty")]
    MissingKty,
    #[error("jwk missing required field: {0}")]
    MissingField(&'static str),
    #[error("jwk kty {0} is not supported")]
    UnsupportedKty(String),
    #[error("canonical JWK could not be serialized: {0}")]
    Serialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests;
