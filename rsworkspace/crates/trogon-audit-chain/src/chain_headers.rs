//! JetStream header names and codec for carrying chain links on a message.
//!
//! Following ADR 0013's convention of reserving `Trogon-`-prefixed
//! application headers for event metadata, the chain carries two headers on
//! every published message:
//!
//! - `Trogon-Chain-Hash`: this entry's [`crate::ChainHash`].
//! - `Trogon-Chain-Previous-Hash`: the previous entry's [`crate::ChainHash`]
//!   (or [`crate::ChainHash::genesis`] for the first message on the subject).
//!
//! Carrying both hashes on every message (rather than only the new hash) lets
//! a verifier reconstruct and check the chain from a single pass over raw
//! JetStream messages, without a prior read to learn what the previous
//! message's hash was.

use async_nats::HeaderMap;

use crate::chain_hash::{ChainHash, ChainHashParseError};

/// Header carrying this entry's chain hash.
pub const CHAIN_HASH_HEADER: &str = "Trogon-Chain-Hash";
/// Header carrying the previous entry's chain hash.
pub const CHAIN_PREVIOUS_HASH_HEADER: &str = "Trogon-Chain-Previous-Hash";

/// Writes the chain headers for one entry into `headers`.
pub fn write_chain_headers(headers: &mut HeaderMap, previous_hash: &ChainHash, hash: &ChainHash) {
    headers.insert(CHAIN_PREVIOUS_HASH_HEADER, previous_hash.as_str());
    headers.insert(CHAIN_HASH_HEADER, hash.as_str());
}

/// Reads and parses both chain headers from `headers`.
pub fn read_chain_headers(headers: &HeaderMap) -> Result<(ChainHash, ChainHash), ChainHeaderError> {
    let previous_hash = read_header(headers, CHAIN_PREVIOUS_HASH_HEADER)?;
    let hash = read_header(headers, CHAIN_HASH_HEADER)?;
    Ok((previous_hash, hash))
}

fn read_header(headers: &HeaderMap, name: &'static str) -> Result<ChainHash, ChainHeaderError> {
    let value = headers
        .get(name)
        .ok_or(ChainHeaderError::Missing { header_name: name })?;
    ChainHash::parse(value.as_str()).map_err(|source| ChainHeaderError::Invalid {
        header_name: name,
        source,
    })
}

/// Error returned when reading chain headers back from a JetStream message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainHeaderError {
    /// A required chain header was absent.
    #[error("message is missing required chain header '{header_name}'")]
    Missing {
        /// The name of the missing header.
        header_name: &'static str,
    },
    /// A chain header was present but not a valid [`ChainHash`].
    #[error("chain header '{header_name}' is not a valid chain hash: {source}")]
    Invalid {
        /// The name of the invalid header.
        header_name: &'static str,
        /// The underlying parse failure.
        #[source]
        source: ChainHashParseError,
    },
}

#[cfg(test)]
mod tests;
