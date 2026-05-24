use std::fmt;

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
        KeyPair::from_public_key(trimmed).map_err(|e| {
            AuthCalloutError::WireFormat(format!("invalid NKey public key: {e}"))
        })?;
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn verify_jwt_issuer(&self, token: &str) -> Result<nats_jwt_rs::Claims<nats_jwt_rs::authorization::AuthRequest>, AuthCalloutError> {
        let claims = nats_jwt_rs::Claims::<nats_jwt_rs::authorization::AuthRequest>::decode(token)
            .map_err(|e| AuthCalloutError::WireFormat(format!("decode authorization request JWT: {e}")))?;
        if claims.iss != self.0 {
            return Err(AuthCalloutError::WireFormat(format!(
                "authorization request issuer mismatch: expected {}, got {}",
                self.0, claims.iss
            )));
        }
        if claims.aud.as_deref() != Some(super::AUTH_REQUEST_AUDIENCE) {
            return Err(AuthCalloutError::WireFormat(format!(
                "authorization request audience must be {}",
                super::AUTH_REQUEST_AUDIENCE
            )));
        }
        Ok(claims)
    }
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
