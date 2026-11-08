use std::fmt;

use nkeys::XKey;
use serde::{Deserialize, Serialize};

use crate::error::AuthCalloutError;

/// NATS XKey (curve25519) public key used for auth-callout payload encryption.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct XkeyPublic(String);

impl TryFrom<String> for XkeyPublic {
    type Error = AuthCalloutError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl XkeyPublic {
    pub(crate) fn from_xkey(key: &XKey) -> Self {
        Self(key.public_key())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AuthCalloutError::WireFormat("XKey public key must be non-empty".into()));
        }
        XKey::from_public_key(trimmed)
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid XKey public key: {e}")))?;
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_xkey(&self) -> Result<XKey, AuthCalloutError> {
        XKey::from_public_key(self.as_str())
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid XKey public key: {e}")))
    }
}

impl fmt::Debug for XkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("XkeyPublic").field(&self.0).finish()
    }
}

impl fmt::Display for XkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
