use super::record::CustodyRecord;

/// A custody record and the revision it was read at, so an update lands only if nobody else changed it.
pub(super) struct Stored {
    pub(super) record: CustodyRecord,
    pub(super) revision: u64,
}
