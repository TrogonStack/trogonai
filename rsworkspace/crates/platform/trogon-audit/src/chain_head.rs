use crate::record_hash::RecordHash;

/// The last record of a writer's chain that the stream acknowledged, and where
/// the stream stored it. A checkpoint signs this, so it moves only on an
/// acknowledgement and never on a publish that might not have landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    sequence: u64,
    hash: RecordHash,
    stream: String,
    stream_sequence: u64,
}

impl ChainHead {
    pub fn new(sequence: u64, hash: RecordHash, stream: impl Into<String>, stream_sequence: u64) -> Self {
        Self {
            sequence,
            hash,
            stream: stream.into(),
            stream_sequence,
        }
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn hash(&self) -> RecordHash {
        self.hash
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn stream_sequence(&self) -> u64 {
        self.stream_sequence
    }
}

#[cfg(test)]
mod tests;
