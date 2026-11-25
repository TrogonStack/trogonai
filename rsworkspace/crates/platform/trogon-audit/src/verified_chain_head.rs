use crate::chain_id::ChainId;
use crate::chain_key::ChainKey;
use crate::record_hash::RecordHash;
use crate::writer_id::WriterId;

/// The last record of a chain range that verified: where the chain stood after
/// it.
///
/// Only a successful verification builds one, so a range can continue from it
/// and a checkpoint can be compared with it without trusting a head a caller
/// typed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedChainHead {
    key: ChainKey,
    sequence: u64,
    hash: RecordHash,
    stream_sequence: u64,
}

impl VerifiedChainHead {
    pub(crate) fn new(key: ChainKey, sequence: u64, hash: RecordHash, stream_sequence: u64) -> Self {
        Self {
            key,
            sequence,
            hash,
            stream_sequence,
        }
    }

    pub fn key(&self) -> &ChainKey {
        &self.key
    }

    pub fn writer(&self) -> &WriterId {
        self.key.writer()
    }

    pub fn chain(&self) -> ChainId {
        self.key.chain()
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn hash(&self) -> RecordHash {
        self.hash
    }

    /// The `AUDIT` stream sequence of the head. When the head was stored more
    /// than once with identical bytes, it is the last copy's: the writer retries
    /// until a copy is acknowledged, so the last copy is the one whose
    /// acknowledgement the writer saw.
    pub fn stream_sequence(&self) -> u64 {
        self.stream_sequence
    }

    pub(crate) fn restored_at(mut self, stream_sequence: u64) -> Self {
        self.stream_sequence = stream_sequence;
        self
    }
}

#[cfg(test)]
mod tests;
