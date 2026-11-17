use std::fmt;
use std::sync::Arc;

use nkeys::{KeyPair, KeyPairType};

use super::TenantAccountError;

/// The tenant a release is asked for, named by its NATS account public key.
///
/// Inside PLATFORM the release subject carries the tenant as its first token
/// (ADR#0081 Decision 3), so only a value that names an account may take that
/// place.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TenantAccount(String);

impl TenantAccount {
    pub fn parse(raw: &str) -> Result<Self, TenantAccountError> {
        let key = KeyPair::from_public_key(raw).map_err(|error| TenantAccountError::NotAPublicKey(Arc::new(error)))?;
        if key.key_pair_type() != KeyPairType::Account {
            return Err(TenantAccountError::NotAnAccount);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
