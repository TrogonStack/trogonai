use super::record::CustodyRecord;

/// A custody record with the KV revision it was read or written at.
pub(crate) struct StoredCustody {
    pub(crate) record: CustodyRecord,
    pub(crate) revision: u64,
}
