use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::DigestError;
use crate::constants::{DIGEST_HASH_BYTES, DIGEST_PREFIX};

/// A SHA-256 digest with exactly 64 lowercase ASCII hexadecimal characters.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Digest(String);

impl Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, DigestError> {
        let value = value.into();
        Self::from_str(&value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 64-character hex hash, without the `sha256:` prefix.
    pub fn hex(&self) -> &str {
        self.0.strip_prefix(DIGEST_PREFIX).unwrap_or(&self.0)
    }
}

impl FromStr for Digest {
    type Err = DigestError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(hash) = value.strip_prefix(DIGEST_PREFIX) else {
            return Err(DigestError::MissingPrefix);
        };
        if hash.is_empty()
            || hash.len() != DIGEST_HASH_BYTES
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(DigestError::InvalidHash);
        }
        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for Digest {
    type Error = DigestError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_str(&value)
    }
}

impl From<Digest> for String {
    fn from(value: Digest) -> Self {
        value.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
