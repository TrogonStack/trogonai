//! Typed report describing the first place a hash chain fails to verify.

use crate::chain_hash::ChainHash;
use crate::chain_sequence::ChainSequence;

/// Describes the first sequence at which chain verification failed, and why.
///
/// `verify_chain` stops at the first break rather than collecting every
/// downstream failure: once one link is wrong, every subsequent computed
/// hash is expected to diverge too, so reporting only the first break points
/// investigators at the actual tamper point instead of a wall of derived
/// noise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainBreak {
    /// The recomputed hash for this sequence does not match the stored hash.
    ///
    /// This is the general tamper signal: it fires whenever the event bytes,
    /// the stored hash, or the stored previous hash were changed after the
    /// entry was written, as long as the entry's own `previous_hash` still
    /// matched the prior entry (otherwise [`ChainBreak::PreviousHashMismatch`]
    /// fires first).
    #[error("chain break at sequence {sequence}: expected hash {expected}, found {found}")]
    HashMismatch {
        /// The sequence at which the break was detected.
        sequence: ChainSequence,
        /// The hash recomputed from the previous hash and this entry's event bytes.
        expected: ChainHash,
        /// The hash actually stored for this entry.
        found: ChainHash,
    },
    /// This entry's `previous_hash` does not match the prior entry's `hash`.
    ///
    /// This fires for reordered or dropped entries even when each
    /// individual entry's own hash is internally consistent with its own
    /// claimed `previous_hash`.
    #[error("chain break at sequence {sequence}: previous hash mismatch, expected {expected}, found {found}")]
    PreviousHashMismatch {
        /// The sequence at which the break was detected.
        sequence: ChainSequence,
        /// The prior entry's actual hash.
        expected: ChainHash,
        /// The previous hash this entry claims.
        found: ChainHash,
    },
    /// The first entry in the iterator did not start at [`ChainSequence::FIRST`].
    #[error("chain does not start at sequence 1, first observed sequence is {found}")]
    DoesNotStartAtOne {
        /// The sequence of the first entry actually observed.
        found: ChainSequence,
    },
    /// A sequence was skipped between two consecutive entries.
    #[error("chain sequence gap: expected {expected}, found {found}")]
    SequenceGap {
        /// The sequence that should have followed the prior entry.
        expected: ChainSequence,
        /// The sequence actually observed next.
        found: ChainSequence,
    },
}

#[cfg(test)]
mod tests;
