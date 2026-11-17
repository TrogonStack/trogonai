//! Why a chain range did not verify (ADR#0086 Decision 6).
//!
//! Each fault names the stream sequence of the record it stopped at and the
//! positions it expected and saw, and never the record's bytes.
//!
//! | Tampering | Fault |
//! | --- | --- |
//! | A record's body or hash edited | [`Unsealed`](ChainVerificationError::Unsealed) with a hash mismatch |
//! | A record rewritten with a fresh hash | [`BrokenLink`](ChainVerificationError::BrokenLink) at the next record |
//! | Records reordered | [`SequenceGap`](ChainVerificationError::SequenceGap), [`SequenceRegression`](ChainVerificationError::SequenceRegression) or [`StreamSequenceRegression`](ChainVerificationError::StreamSequenceRegression) |
//! | A record deleted inside the range | [`SequenceGap`](ChainVerificationError::SequenceGap) |
//! | The genesis deleted | [`MissingGenesis`](ChainVerificationError::MissingGenesis) |
//! | A different record stored at a used sequence | [`ConflictingDuplicate`](ChainVerificationError::ConflictingDuplicate) |
//! | A header or subject rewritten | [`HeaderMismatch`](ChainVerificationError::HeaderMismatch) or [`SubjectMismatch`](ChainVerificationError::SubjectMismatch) |
//!
//! What a chain alone cannot show:
//!
//! - deleting the tail of a chain: the remaining records still link, so only a
//!   signed checkpoint naming a later head reveals it;
//! - any order across writers or across chains of one writer: chains are
//!   independent;
//! - that a reset was authorized: the genesis is not signed, so it shows only that
//!   a chain began;
//! - a record dropped before the writer numbered it.

use async_nats::HeaderName;
use buffa::DecodeError;

use crate::chain_id::ChainId;
use crate::constants::{GENESIS_SEQUENCE, MAX_CHAIN_RANGE_RECORDS};
use crate::seal_error::SealError;
use crate::writer_id::WriterId;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainVerificationError {
    /// A range with no record proves nothing, so it never yields a head.
    #[error("chain range holds no record")]
    EmptyRange,
    #[error("chain range holds more than {MAX_CHAIN_RANGE_RECORDS} records")]
    RangeTooLong,
    #[error(
        "record at stream sequence {stream_sequence} does not come after stream sequence {previous_stream_sequence}"
    )]
    StreamSequenceRegression {
        stream_sequence: u64,
        previous_stream_sequence: u64,
    },
    #[error("record at stream sequence {stream_sequence} is not sealed")]
    Unsealed {
        stream_sequence: u64,
        #[source]
        source: SealError,
    },
    #[error("record at stream sequence {stream_sequence} is not an audit record")]
    Malformed {
        stream_sequence: u64,
        #[source]
        source: DecodeError,
    },
    #[error("record at stream sequence {stream_sequence} has no integrity")]
    MissingIntegrity { stream_sequence: u64 },
    #[error("record at stream sequence {stream_sequence} carries its record hash inside the hashed bytes")]
    HashInBody { stream_sequence: u64 },
    #[error("record at stream sequence {stream_sequence} is not from writer {expected}")]
    WrongWriter { stream_sequence: u64, expected: WriterId },
    #[error("record at stream sequence {stream_sequence} has no valid chain id")]
    InvalidChainId { stream_sequence: u64 },
    #[error("record at stream sequence {stream_sequence} is on chain {observed}, not chain {expected}")]
    WrongChain {
        stream_sequence: u64,
        expected: ChainId,
        observed: ChainId,
    },
    #[error("record at stream sequence {stream_sequence} has a {header} header that disagrees with its payload")]
    HeaderMismatch { stream_sequence: u64, header: HeaderName },
    #[error("record at stream sequence {stream_sequence} is on a subject that disagrees with its payload")]
    SubjectMismatch { stream_sequence: u64 },
    #[error(
        "chain must open at sequence {GENESIS_SEQUENCE}, but record at stream sequence {stream_sequence} is sequence {observed}"
    )]
    MissingGenesis { stream_sequence: u64, observed: u64 },
    #[error("genesis at stream sequence {stream_sequence} carries a previous hash")]
    GenesisPreviousHash { stream_sequence: u64 },
    #[error("genesis at stream sequence {stream_sequence} is not a writer lifecycle record that starts a chain")]
    GenesisNotLifecycle { stream_sequence: u64 },
    #[error("expected sequence {expected}, but record at stream sequence {stream_sequence} is sequence {observed}")]
    SequenceGap {
        stream_sequence: u64,
        expected: u64,
        observed: u64,
    },
    #[error(
        "expected sequence {expected}, but record at stream sequence {stream_sequence} goes back to sequence {observed}"
    )]
    SequenceRegression {
        stream_sequence: u64,
        expected: u64,
        observed: u64,
    },
    #[error("record at stream sequence {stream_sequence} follows sequence {previous}, the last a chain can hold")]
    SequenceOverflow { stream_sequence: u64, previous: u64 },
    #[error(
        "record at stream sequence {stream_sequence} repeats sequence {sequence} from stream sequence {previous_stream_sequence} with different bytes"
    )]
    ConflictingDuplicate {
        stream_sequence: u64,
        sequence: u64,
        previous_stream_sequence: u64,
    },
    #[error("record at stream sequence {stream_sequence}, sequence {sequence}, has no previous hash")]
    MissingPreviousHash { stream_sequence: u64, sequence: u64 },
    #[error(
        "record at stream sequence {stream_sequence}, sequence {sequence}, does not link to the hash of the record before it"
    )]
    BrokenLink { stream_sequence: u64, sequence: u64 },
}

#[cfg(test)]
mod tests;
