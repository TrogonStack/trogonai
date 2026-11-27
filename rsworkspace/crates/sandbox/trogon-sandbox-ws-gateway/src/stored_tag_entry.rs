use serde::Deserialize;
use trogon_sandbox_contract::{SnapshotTag, SnapshotTagRecord};

/// Snapshot tag KV value: the record plus the owner stamp current brokers write (absent on older records).
#[derive(Deserialize)]
pub(crate) struct StoredTagEntry {
    #[serde(flatten)]
    pub(crate) record: SnapshotTagRecord,
    #[serde(default)]
    pub(crate) owner: Option<SnapshotTag>,
}
