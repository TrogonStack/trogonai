use std::fmt;

use nkeys::{KeyPair, XKey};

use crate::error::AuthCalloutError;

/// NATS NKey or XKey seed (sensitive; load from env/file at process edge only).
#[derive(Clone)]
pub struct NkeySeed(String);

impl NkeySeed {
    pub fn parse(seed: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let seed = seed.into();
        let trimmed = seed.trim();
        if trimmed.is_empty() {
            return Err(AuthCalloutError::WireFormat("NKey seed must be non-empty".into()));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_signing_keypair(&self) -> Result<KeyPair, AuthCalloutError> {
        KeyPair::from_seed(self.as_str())
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid NKey seed: {e}")))
    }

    pub fn to_xkey(&self) -> Result<XKey, AuthCalloutError> {
        XKey::from_seed(self.as_str())
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid XKey seed: {e}")))
    }
}

impl fmt::Debug for NkeySeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NkeySeed([redacted])")
    }
}
