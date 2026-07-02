//! Chain extension: the core hash-linking operation.

use crate::canonical_event_bytes::CanonicalEventBytes;
use crate::chain_hash::ChainHash;

/// Computes the next chain hash from the previous hash and one event's
/// canonical bytes.
///
/// The link is `SHA-256(previous_hash_hex_bytes || canonical_event_bytes)`.
/// Hashing the previous hash's hex string (rather than its raw digest bytes)
/// keeps the construction simple to reproduce from the header value alone: a
/// verifier that only has the hex string on the wire does not need to decode
/// it back to raw bytes before extending the chain.
///
/// This mirrors AUDIT-COMPLIANCE-1.0 Section 9.2's linear chain
/// (`entry_hash` derived from a structure that includes `previous_hash`),
/// specialized to a single opaque canonical event payload instead of a fixed
/// audit-entry schema.
pub fn extend(previous: &ChainHash, event_bytes: &CanonicalEventBytes) -> ChainHash {
    let mut buffer = Vec::with_capacity(previous.as_str().len() + event_bytes.as_bytes().len());
    buffer.extend_from_slice(previous.as_str().as_bytes());
    buffer.extend_from_slice(event_bytes.as_bytes());
    ChainHash::digest(&buffer)
}

#[cfg(test)]
mod tests;
