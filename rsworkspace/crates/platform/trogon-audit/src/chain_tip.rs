use crate::chain_id::ChainId;
use crate::pending_record::PendingRecord;
use crate::record::Record;
use crate::record_hash::RecordHash;
use crate::sealed_record::SealedRecord;
use crate::writer_id::WriterId;

/// The end of a writer's chain: the last record numbered, and that record while
/// it waits to be stored.
///
/// A record is numbered only when nothing is pending, so at most one numbered
/// record is ever unacknowledged and the stream stores the chain in order.
#[derive(Debug)]
pub(crate) struct ChainTip {
    sequence: u64,
    hash: RecordHash,
    pub(crate) pending: Option<PendingRecord>,
}

impl ChainTip {
    /// A chain whose first record is `genesis`, not yet stored.
    pub(crate) fn opened_by(genesis: SealedRecord) -> Self {
        Self {
            sequence: genesis.sequence(),
            hash: genesis.hash(),
            pending: Some(PendingRecord::new(genesis)),
        }
    }

    /// Numbers `record` after the last numbered record, links it to that record's
    /// hash and makes it pending. A chain at the last sequence cannot take
    /// another record, and hands it back.
    pub(crate) fn append(&mut self, record: Record, writer: &WriterId, chain: ChainId) -> Result<(), Box<Record>> {
        debug_assert!(
            self.pending.is_none(),
            "a record is numbered only when nothing is pending"
        );
        let Some(sequence) = self.sequence.checked_add(1) else {
            return Err(Box::new(record));
        };
        let sealed = SealedRecord::seal(record, writer, chain, sequence, Some(self.hash));
        self.sequence = sequence;
        self.hash = sealed.hash();
        self.pending = Some(PendingRecord::new(sealed));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn at(sequence: u64, hash: RecordHash) -> Self {
        Self {
            sequence,
            hash,
            pending: None,
        }
    }
}

#[cfg(test)]
mod tests;
