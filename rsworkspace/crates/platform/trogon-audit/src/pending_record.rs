use tokio::time::Instant;

use crate::sealed_record::SealedRecord;

/// The record numbered last, waiting for the stream to acknowledge it. It stays
/// until it is stored, retried with the same bytes and message id, because the
/// next record already names its hash.
#[derive(Debug)]
pub(crate) struct PendingRecord {
    pub(crate) record: SealedRecord,
    /// When it was first published, so a retry made after the stream's duplicate
    /// window can be counted as one that may store a second copy.
    pub(crate) first_attempt: Option<Instant>,
}

impl PendingRecord {
    pub(crate) fn new(record: SealedRecord) -> Self {
        Self {
            record,
            first_attempt: None,
        }
    }
}
