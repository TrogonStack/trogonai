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
        KeyPair::from_public_key(trimmed)
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid NKey public key: {e}")))?;
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

#[cfg(test)]
mod tests;
