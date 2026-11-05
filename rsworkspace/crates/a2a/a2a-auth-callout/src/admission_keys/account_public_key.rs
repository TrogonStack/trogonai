use std::fmt;
use std::str::FromStr;

use nkeys::{KeyPair, KeyPairType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccountPublicKeyError {
    #[error("is not an NKey public key")]
    NotAPublicKey,
    #[error("is an NKey public key that names something other than an account")]
    NotAnAccount,
}

fn account_public_key(raw: &str) -> Result<String, AccountPublicKeyError> {
    let trimmed = raw.trim();
    let key = KeyPair::from_public_key(trimmed).map_err(|_| AccountPublicKeyError::NotAPublicKey)?;
    if key.key_pair_type() != KeyPairType::Account {
        return Err(AccountPublicKeyError::NotAnAccount);
    }
    Ok(trimmed.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantAccount(String);

impl TenantAccount {
    pub fn parse(raw: &str) -> Result<Self, AccountPublicKeyError> {
        account_public_key(raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for TenantAccount {
    type Err = AccountPublicKeyError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl fmt::Display for TenantAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthAccount(String);

impl AuthAccount {
    pub fn parse(raw: &str) -> Result<Self, AccountPublicKeyError> {
        account_public_key(raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AuthAccount {
    type Err = AccountPublicKeyError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl fmt::Display for AuthAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
