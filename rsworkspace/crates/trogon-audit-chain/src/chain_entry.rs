//! One position in an audit hash chain, as read back for verification.

use crate::canonical_event_bytes::CanonicalEventBytes;
use crate::chain_hash::ChainHash;
use crate::chain_sequence::ChainSequence;

/// One observed position in a hash chain, as recovered from durable storage
/// (JetStream headers, a companion KV bucket, or an in-memory fixture in
/// tests).
///
/// `ChainEntry` carries the claimed `previous_hash` and `hash` alongside the
/// event bytes so [`crate::verify_chain`] can recompute the link with
/// [`crate::extend`] and compare it against what was stored, without needing
/// to know anything about how the entry was transported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainEntry {
    sequence: ChainSequence,
    previous_hash: ChainHash,
    event_bytes: CanonicalEventBytes,
    hash: ChainHash,
}

impl ChainEntry {
    /// Builds a chain entry from its observed fields.
    ///
    /// This does not validate that `hash` actually equals
    /// `extend(previous_hash, event_bytes)`; that check is
    /// [`crate::verify_chain`]'s job, not construction's. A `ChainEntry` is a
    /// faithful record of what was read, tampered or not.
    pub fn new(
        sequence: ChainSequence,
        previous_hash: ChainHash,
        event_bytes: CanonicalEventBytes,
        hash: ChainHash,
    ) -> Self {
        Self {
            sequence,
            previous_hash,
            event_bytes,
            hash,
        }
    }

    /// The entry's position in the chain.
    pub fn sequence(&self) -> ChainSequence {
        self.sequence
    }

    /// The previous hash claimed by this entry.
    pub fn previous_hash(&self) -> &ChainHash {
        &self.previous_hash
    }

    /// The canonical event bytes this entry commits to.
    pub fn event_bytes(&self) -> &CanonicalEventBytes {
        &self.event_bytes
    }

    /// The hash claimed by this entry.
    pub fn hash(&self) -> &ChainHash {
        &self.hash
    }
}

#[cfg(test)]
mod tests;
