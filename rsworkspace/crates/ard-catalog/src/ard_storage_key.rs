//! NATS-safe storage key derived from an [`ArdIdentifier`].

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::ard_identifier::ArdIdentifier;

/// Lowercase hex SHA-256 of the full ARD identifier string.
///
/// Safe for NATS KV keys and single subject tokens because it is ASCII,
/// dot-free, and wildcard-free.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArdStorageKey(Arc<str>);

impl ArdStorageKey {
    pub fn from_identifier(identifier: &ArdIdentifier) -> Self {
        let digest = Sha256::digest(identifier.as_str().as_bytes());
        Self(Arc::from(hex::encode(digest)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ArdStorageKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests;
