use sha2::{Digest, Sha256};

use crate::constants::RECORD_HASH_BYTES;

/// The SHA-256 of a record's exact encoded bytes, with `record_hash` absent
/// (ADR#0086 Decision 6). The next record in the chain carries it as
/// `previous_hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordHash([u8; RECORD_HASH_BYTES]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecordHashError {
    #[error("record hash must be {RECORD_HASH_BYTES} bytes, got {0}")]
    WrongLength(usize),
}

impl RecordHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn new(bytes: [u8; RECORD_HASH_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, RecordHashError> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| RecordHashError::WrongLength(bytes.len()))
    }

    pub fn as_bytes(&self) -> &[u8; RECORD_HASH_BYTES] {
        &self.0
    }
}

#[cfg(test)]
mod tests;
