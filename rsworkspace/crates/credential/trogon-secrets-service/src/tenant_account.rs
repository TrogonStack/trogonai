use std::fmt;

use nkeys::{KeyPair, KeyPairType};

/// The account a release request was imported from.
///
/// The NATS caller reads it from the importing account token, never from a
/// request field, so it is the only tenant identity the release core trusts.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TenantAccount(String);

impl TenantAccount {
    pub fn parse(raw: &str) -> Result<Self, TenantAccountError> {
        let key = KeyPair::from_public_key(raw).map_err(|_| TenantAccountError::NotAPublicKey)?;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TenantAccountError {
    #[error("tenant account is not an NKey public key")]
    NotAPublicKey,
    #[error("tenant account is an NKey public key that names something other than an account")]
    NotAnAccount,
}

#[cfg(test)]
mod tests;
