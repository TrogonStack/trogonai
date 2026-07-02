//! Fold-based verification of an ordered sequence of chain entries.

use crate::chain_break::ChainBreak;
use crate::chain_entry::ChainEntry;
use crate::chain_hash::ChainHash;
use crate::chain_sequence::ChainSequence;
use crate::extend::extend;

/// Verifies that an ordered iterator of [`ChainEntry`] values forms an
/// unbroken hash chain starting from [`ChainHash::genesis`].
///
/// This is a fold, not a batch collect-and-compare: it walks the iterator
/// once, recomputing each link with [`extend`] and comparing it against the
/// stored hash and the stored `previous_hash`, and returns as soon as it
/// finds the first inconsistency. Iteration order is caller-defined; callers
/// reading from JetStream must supply entries in ascending stream sequence
/// order for the sequence checks to be meaningful.
///
/// Returns `Ok(())` when every entry's stored hash and previous-hash link up
/// correctly starting from an empty chain, or the first [`ChainBreak`]
/// encountered otherwise.
pub fn verify_chain<I>(entries: I) -> Result<(), ChainBreak>
where
    I: IntoIterator<Item = ChainEntry>,
{
    let mut expected_sequence = ChainSequence::FIRST;
    let mut expected_previous_hash = ChainHash::genesis();

    for entry in entries {
        if entry.sequence() != expected_sequence {
            return Err(first_sequence_error(expected_sequence, entry.sequence()));
        }

        if entry.previous_hash() != &expected_previous_hash {
            return Err(ChainBreak::PreviousHashMismatch {
                sequence: entry.sequence(),
                expected: expected_previous_hash,
                found: entry.previous_hash().clone(),
            });
        }

        let recomputed = extend(entry.previous_hash(), entry.event_bytes());
        if &recomputed != entry.hash() {
            return Err(ChainBreak::HashMismatch {
                sequence: entry.sequence(),
                expected: recomputed,
                found: entry.hash().clone(),
            });
        }

        expected_previous_hash = entry.hash().clone();
        expected_sequence = expected_sequence.next();
    }

    Ok(())
}

fn first_sequence_error(expected: ChainSequence, found: ChainSequence) -> ChainBreak {
    if expected == ChainSequence::FIRST {
        ChainBreak::DoesNotStartAtOne { found }
    } else {
        ChainBreak::SequenceGap { expected, found }
    }
}

#[cfg(test)]
mod tests;
