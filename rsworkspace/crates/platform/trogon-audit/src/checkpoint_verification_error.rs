//! Why a checkpoint did not verify (ADR#0086 Decision 6).
//!
//! The signature is checked before the statement is read, so every fault after
//! [`BadSignature`](CheckpointVerificationError::BadSignature) is in bytes a
//! trusted key signed: a signer's mistake or a mismatched head, not tampering.

use buffa::DecodeError;

use crate::chain_id::ChainId;
use crate::key_id::{KeyId, KeyIdError};
use crate::writer_id::WriterId;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointVerificationError {
    #[error("checkpoint is not an audit checkpoint")]
    Malformed(#[source] DecodeError),
    #[error("checkpoint signature is not a detached JWS")]
    MalformedJws,
    #[error("checkpoint JWS protected header is not the checkpoint header")]
    MalformedHeader,
    #[error("checkpoint JWS is not of the checkpoint type")]
    WrongType,
    #[error("checkpoint JWS algorithm is not EdDSA")]
    AlgorithmNotAllowed,
    #[error("checkpoint JWS key id is not valid")]
    MalformedKeyId(#[source] KeyIdError),
    /// Boxed because a parsed key is large and the other faults are small.
    #[error("checkpoint is signed by {key_id}, which is not trusted")]
    UntrustedKey { key_id: Box<KeyId> },
    #[error("checkpoint JWS signature is not an Ed25519 signature")]
    MalformedSignature,
    #[error("checkpoint signature does not verify")]
    BadSignature,
    #[error("checkpoint statement is not a checkpoint statement")]
    MalformedStatement(#[source] DecodeError),
    #[error("checkpoint statement names a key other than the one that signed it")]
    KeyIdMismatch,
    #[error("checkpoint is not for writer {expected}")]
    WrongWriter { expected: WriterId },
    #[error("checkpoint has no valid chain id")]
    InvalidChainId,
    #[error("checkpoint is for chain {observed}, not chain {expected}")]
    WrongChain { expected: ChainId, observed: ChainId },
    #[error("checkpoint names sequence {observed}, not the verified head's sequence {expected}")]
    HeadSequenceMismatch { expected: u64, observed: u64 },
    #[error("checkpoint names a head hash other than the verified head's at sequence {sequence}")]
    HeadHashMismatch { sequence: u64 },
    #[error("checkpoint names stream sequence {observed}, not the verified head's stream sequence {expected}")]
    StreamSequenceMismatch { expected: u64, observed: u64 },
    #[error("checkpoint has no valid issue time")]
    InvalidIssuedAt,
    #[error("checkpoint was issued outside the accepted window")]
    IssuedOutsideBounds,
    #[error("checkpoint has no valid checkpoint id")]
    InvalidCheckpointId,
    #[error("checkpoint has no valid component")]
    InvalidComponent,
}

#[cfg(test)]
mod tests;
