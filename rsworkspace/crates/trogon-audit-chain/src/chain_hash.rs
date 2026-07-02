//! Chain hash value object.

use std::fmt;
use std::sync::Arc;

use sha2::{Digest, Sha256};

/// Number of lowercase hex characters in a SHA-256 digest (32 bytes * 2).
const HEX_LEN: usize = 64;

/// A SHA-256 digest rendered as a lowercase hex string, used as a link in the
/// audit hash chain.
///
/// Construction always goes through [`ChainHash::genesis`] or
/// [`ChainHash::digest`], so every instance is a well-formed 64-character
/// lowercase hex string. There is no fallible parser here on purpose: chain
/// hashes are produced locally, never accepted as untrusted input directly as
/// a `ChainHash` (see [`crate::ChainHeaderError`] for the header-parsing
/// boundary that validates untrusted wire data).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChainHash(Arc<str>);

impl ChainHash {
    /// The distinguished hash that precedes the first entry in a chain.
    ///
    /// Using an explicit sentinel (rather than an `Option<ChainHash>` at every
    /// call site) keeps `extend` total: genesis is just another previous hash.
    pub fn genesis() -> Self {
        Self(Arc::from("0".repeat(HEX_LEN)))
    }

    /// Computes the SHA-256 digest of `bytes` and returns it as a [`ChainHash`].
    pub fn digest(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(Arc::from(hex::encode(digest)))
    }

    /// Returns the lowercase hex representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a lowercase hex SHA-256 digest received from an untrusted
    /// source (e.g. a JetStream header value).
    pub fn parse(value: &str) -> Result<Self, ChainHashParseError> {
        if value.len() != HEX_LEN {
            return Err(ChainHashParseError::WrongLength { found: value.len() });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ChainHashParseError::NotLowercaseHex);
        }
        Ok(Self(Arc::from(value)))
    }
}

impl fmt::Display for ChainHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Error returned when parsing untrusted input into a [`ChainHash`] fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainHashParseError {
    /// The input was not exactly 64 characters long.
    #[error("chain hash must be {HEX_LEN} hex characters, found {found}")]
    WrongLength {
        /// The number of characters actually found.
        found: usize,
    },
    /// The input contained characters outside `[0-9a-f]`.
    #[error("chain hash must be lowercase hex")]
    NotLowercaseHex,
}

#[cfg(test)]
mod tests;
