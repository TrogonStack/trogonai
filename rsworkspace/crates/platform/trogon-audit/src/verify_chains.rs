//! Verifies every chain in a range of the `AUDIT` stream that interleaves
//! writers and their chains.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use buffa::Message;
use trogonai_proto::platform::audit::v1::AuditRecord;

use crate::chain_anchor::ChainAnchor;
use crate::chain_id::ChainId;
use crate::chain_key::ChainKey;
use crate::chain_outcome::ChainOutcome;
use crate::chain_verification_error::ChainVerificationError;
use crate::chains_verification::ChainsVerification;
use crate::constants::MAX_CHAIN_RANGE_RECORDS;
use crate::stored_audit_record::StoredAuditRecord;
use crate::verified_chain_head::VerifiedChainHead;
use crate::verify_chain_range::verify_chain_range;
use crate::writer_id::WriterId;

/// Splits `records`, stream order kept, by writer and chain, and verifies each
/// chain with [`verify_chain_range`].
///
/// A chain continues from its head in `trusted` and otherwise must open with its
/// genesis inside the range. When `trusted` holds two heads of one chain, the
/// one with the higher sequence is used, whatever their order. A message is
/// placed by the writer and chain its payload names, or by its
/// `Trogon-Audit-Writer` and `Trogon-Audit-Chain` headers when the payload does
/// not decode; the chain's own walk then checks it in full. The stream holds no
/// order across writers, so none is checked.
pub fn verify_chains(
    trusted: impl IntoIterator<Item = VerifiedChainHead>,
    records: impl IntoIterator<Item = StoredAuditRecord>,
) -> Result<ChainsVerification, ChainVerificationError> {
    let mut partitions: Vec<(ChainKey, Vec<StoredAuditRecord>)> = Vec::new();
    let mut index: HashMap<ChainKey, usize> = HashMap::new();
    let mut unattributed = Vec::new();
    let mut total = 0usize;
    for record in records {
        if total == MAX_CHAIN_RANGE_RECORDS {
            return Err(ChainVerificationError::RangeTooLong);
        }
        total += 1;
        let Some(key) = chain_key(&record) else {
            unattributed.push(record.stream_sequence());
            continue;
        };
        let position = *index.entry(key.clone()).or_insert_with(|| {
            partitions.push((key, Vec::new()));
            partitions.len() - 1
        });
        partitions[position].1.push(record);
    }
    if total == 0 {
        return Err(ChainVerificationError::EmptyRange);
    }

    let mut heads: HashMap<ChainKey, VerifiedChainHead> = HashMap::new();
    for head in trusted {
        match heads.entry(head.key().clone()) {
            Entry::Occupied(mut kept) if head.sequence() > kept.get().sequence() => {
                kept.insert(head);
            }
            Entry::Occupied(_) => {}
            Entry::Vacant(slot) => {
                slot.insert(head);
            }
        }
    }
    let chains = partitions
        .into_iter()
        .map(|(key, records)| {
            let anchor = match heads.remove(&key) {
                Some(head) => ChainAnchor::TrustedHead(head),
                None => ChainAnchor::Genesis {
                    writer: key.writer().clone(),
                    chain: key.chain(),
                },
            };
            ChainOutcome::new(key, verify_chain_range(anchor, records))
        })
        .collect();
    Ok(ChainsVerification::new(chains, unattributed))
}

fn chain_key(record: &StoredAuditRecord) -> Option<ChainKey> {
    payload_key(record).or_else(|| header_key(record))
}

/// The full stored payload decodes as a record with the trailer merged in, so
/// the key is read without first checking the seal.
fn payload_key(record: &StoredAuditRecord) -> Option<ChainKey> {
    let message = AuditRecord::decode_from_slice(record.payload()).ok()?;
    let integrity = message.integrity.as_option()?;
    key(&integrity.writer_id, integrity.chain_id.as_deref()?)
}

fn header_key(record: &StoredAuditRecord) -> Option<ChainKey> {
    key(record.writer()?, record.chain()?)
}

fn key(writer: &str, chain: &str) -> Option<ChainKey> {
    let writer = WriterId::new(writer).ok()?;
    let chain = ChainId::parse(chain).ok()?;
    Some(ChainKey::new(writer, chain))
}

#[cfg(test)]
mod tests;
