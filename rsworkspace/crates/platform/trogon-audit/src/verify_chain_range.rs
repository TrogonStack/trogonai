//! Verifies one writer's chain over a range of stored records (ADR#0086
//! Decision 6), offline and without a NATS client.

use crate::chain_anchor::ChainAnchor;
use crate::chain_verification_error::ChainVerificationError;
use crate::chain_walk::ChainWalk;
use crate::stored_audit_record::StoredAuditRecord;
use crate::verified_chain_range::VerifiedChainRange;

/// Checks `records`, one chain's messages in the order the stream stored them,
/// from `anchor`, and returns the head they end at.
///
/// Each record must be sealed and hash to its trailer, carry no record hash
/// inside the hashed bytes, belong to the anchor's writer and chain, follow the
/// previous record's sequence by exactly one and name its hash, sit later in the
/// stream than the previous record, and agree with whatever subject and headers
/// the reader supplied. An identical copy of the previous record is a
/// [`ChainNotice`](crate::ChainNotice), not a fault. The first fault stops the
/// walk; the input is never sorted or deduplicated to hide one.
pub fn verify_chain_range(
    anchor: ChainAnchor,
    records: impl IntoIterator<Item = StoredAuditRecord>,
) -> Result<VerifiedChainRange, ChainVerificationError> {
    let mut walk = ChainWalk::new(anchor);
    for record in records {
        walk.check(&record)?;
    }
    walk.finish()
}

#[cfg(test)]
mod tests;
