use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

use crate::{ApiKeyHashError, BearerToken};

const HASH_LEN: usize = 64;

/// The lowercase hexadecimal SHA-256 of a bearer token, which is the key records are stored under.
///
/// It holds exactly 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ApiKeyHash(String);

impl ApiKeyHash {
    pub fn of(token: &BearerToken) -> Self {
        Self(hex::encode(Sha256::digest(token.expose_secret().as_bytes())))
    }

    pub fn new(value: impl Into<String>) -> Result<Self, ApiKeyHashError> {
        let value = value.into();
        if value.len() != HASH_LEN {
            return Err(ApiKeyHashError::WrongLength);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(ApiKeyHashError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ApiKeyHash {
    type Err = ApiKeyHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for ApiKeyHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
