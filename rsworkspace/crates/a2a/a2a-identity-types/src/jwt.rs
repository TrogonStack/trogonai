use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::JwtError;

/// Header name carrying a serialized [`CallerJwtHeaderValue`] on every A2A
/// request, including gateway-mediated traffic.
pub const CALLER_JWT_HEADER_NAME: &str = "A2a-Caller-Jwt";

/// Compact JWT string suitable for header transport. Validates shape on
/// construction (3 dotted segments) without verifying signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerJwtHeaderValue(String);

impl CallerJwtHeaderValue {
    /// Builds a header value from a [`MintedUserJwt`]. The minted JWT is already
    /// shape-validated at construction, so this is infallible.
    pub fn from_minted(jwt: &MintedUserJwt) -> Self {
        Self(jwt.as_str().to_owned())
    }

    pub fn parse(token: impl Into<String>) -> Result<Self, JwtError> {
        let token = token.into();
        validate_compact_jwt_shape(&token)?;
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_compact_jwt_shape(token: &str) -> Result<(), JwtError> {
    if token.trim().is_empty() {
        return Err(JwtError::Decode("caller JWT header value is empty".into()));
    }
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::Decode("caller JWT header value is not a compact JWT".into()));
    }
    if parts.iter().any(|p| p.is_empty()) {
        return Err(JwtError::Decode("caller JWT header value has empty segment".into()));
    }
    Ok(())
}

impl fmt::Display for CallerJwtHeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// User JWT minted for bridge/gateway consumption; carried as the inner
/// `nats.jwt` on wire responses. Validates shape on construction but does not
/// verify the signature — that lives gateway-side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedUserJwt(String);

impl MintedUserJwt {
    /// Constructs a minted JWT from an already-shape-valid compact JWT string.
    /// Returns an error if the input is not three non-empty `.`-separated
    /// segments. Signature verification still lives gateway-side.
    pub fn new(token: impl Into<String>) -> Result<Self, JwtError> {
        let token = token.into();
        validate_compact_jwt_shape(&token)?;
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// Decodes the payload (without signature verification) and checks `exp`
    /// and `nbf` are valid right now. Returns Ok if the token is still fresh.
    pub fn ensure_fresh(&self) -> Result<(), JwtError> {
        let payload = decode_nats_user_payload(self.as_str())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(JwtError::SystemTime)?
            .as_secs();
        let now_i64 = i64::try_from(now).map_err(|_| JwtError::IssuedAtOutOfRange)?;
        let exp = payload
            .get("exp")
            .and_then(Value::as_i64)
            .ok_or_else(|| JwtError::Decode("user JWT missing exp".into()))?;
        if exp <= now_i64 {
            return Err(JwtError::Decode("user JWT expired".into()));
        }
        if let Some(nbf) = payload.get("nbf").and_then(Value::as_i64)
            && nbf > now_i64
        {
            return Err(JwtError::Decode("user JWT not yet valid".into()));
        }
        Ok(())
    }
}

/// Decodes the payload segment of a compact JWT into a serde_json `Value`
/// without verifying the signature. Returns `Err` on malformed input.
pub fn decode_nats_user_payload(token: &str) -> Result<Value, JwtError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::Decode("invalid JWT segment count".into()));
    }
    decode_segment(parts[1])
}

fn decode_segment<T: DeserializeOwned>(input: &str) -> Result<T, JwtError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(input.as_bytes())
        .map_err(|e| JwtError::Decode(e.to_string()))?;
    serde_json::from_slice(&decoded).map_err(|e| JwtError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests;
