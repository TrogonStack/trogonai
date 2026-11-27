use trogon_sandbox_contract::SnapshotTagRecord;

/// Where a tag's record was found and what it holds.
pub(super) struct LocatedTag {
    pub(super) key: String,
    pub(super) record: SnapshotTagRecord,
    pub(super) revision: u64,
}
