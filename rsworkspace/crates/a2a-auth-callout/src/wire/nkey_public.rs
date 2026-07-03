use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::KeyPair;

use crate::error::AuthCalloutError;

/// NATS NKey public identifier (base32-encoded, prefix `A`/`U`/…).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NkeyPublic(String);

impl NkeyPublic {
    pub fn parse(value: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AuthCalloutError::WireFormat("NKey public key must be non-empty".into()));
        }
        KeyPair::from_public_key(trimmed)
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid NKey public key: {e}")))?;
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Verify a `$SYS.REQ.USER.AUTH` request JWT was signed by *this* NkeyPublic.
    /// The previous shape only verified that whatever `iss` the JWT itself
    /// claimed had a matching signature — a forged request signed by an
    /// arbitrary NKey would pass. Now we require the JWT's `iss` to equal the
    /// configured server-issuer NkeyPublic before signature checks even run.
    pub(crate) fn verify_jwt_issuer(&self, token: &str) -> Result<Claims<AuthRequest>, AuthCalloutError> {
        decode_server_auth_request_jwt(token, self)
    }
}

fn decode_server_auth_request_jwt(
    token: &str,
    expected_issuer: &NkeyPublic,
) -> Result<Claims<AuthRequest>, AuthCalloutError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthCalloutError::WireFormat("invalid JWT segment count".into()));
    }

    let signature = parts[2];
    let decoded_sig = URL_SAFE_NO_PAD
        .decode(signature.as_bytes())
        .map_err(|e| AuthCalloutError::WireFormat(format!("JWT signature base64: {e}")))?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .map_err(|e| AuthCalloutError::WireFormat(format!("JWT payload base64: {e}")))?;

    let mut payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| AuthCalloutError::WireFormat(format!("authorization request JWT payload JSON: {e}")))?;

    let iss = payload
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthCalloutError::WireFormat("authorization request JWT missing iss".into()))?;

    // Pin issuer identity *before* verifying the signature — the signature
    // alone proves the holder of *some* signing key signed the token, not
    // that the configured server signed it.
    if iss != expected_issuer.as_str() {
        return Err(AuthCalloutError::WireFormat(format!(
            "authorization request issuer {iss:?} does not match configured server issuer"
        )));
    }

    let kp = KeyPair::from_public_key(iss)
        .map_err(|e| AuthCalloutError::WireFormat(format!("authorization request issuer NKey: {e}")))?;
    kp.verify(&token.as_bytes()[0..token.len() - signature.len() - 1], &decoded_sig)
        .map_err(|e| AuthCalloutError::WireFormat(format!("authorization request JWT signature: {e}")))?;

    normalize_auth_request_payload(&mut payload);

    let claims: Claims<AuthRequest> = serde_json::from_value(payload)
        .map_err(|e| AuthCalloutError::WireFormat(format!("decode authorization request JWT: {e}")))?;

    if claims.aud.as_deref() != Some(super::AUTH_REQUEST_AUDIENCE) {
        return Err(AuthCalloutError::WireFormat(format!(
            "authorization request audience must be {}",
            super::AUTH_REQUEST_AUDIENCE
        )));
    }

    Ok(claims)
}

fn normalize_auth_request_payload(payload: &mut serde_json::Value) {
    let Some(client_info) = payload
        .get_mut("nats")
        .and_then(|nats| nats.get_mut("client_info"))
        .and_then(|ci| ci.as_object_mut())
    else {
        return;
    };
    client_info
        .entry("name_tag")
        .or_insert_with(|| serde_json::Value::String(String::new()));
    client_info
        .entry("nonce")
        .or_insert_with(|| serde_json::Value::String(String::new()));

    let Some(tls) = payload
        .get_mut("nats")
        .and_then(|nats| nats.get_mut("client_tls"))
        .and_then(|tls| tls.as_object_mut())
    else {
        return;
    };
    tls.entry("certs").or_insert_with(|| serde_json::json!([]));
    tls.entry("verified_chains").or_insert_with(|| serde_json::json!([]));
}

impl fmt::Debug for NkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NkeyPublic").field(&self.0).finish()
    }
}

impl fmt::Display for NkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
