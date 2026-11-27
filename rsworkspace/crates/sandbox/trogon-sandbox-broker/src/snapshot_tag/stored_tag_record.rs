use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{SnapshotTag, SnapshotTagRecord};

/// What the KV bucket stores under a tag key. Current writes stamp the owning
/// tag so a reader can tell its own record from one another tag wrote under
/// a colliding legacy key; records from before the stamp carry none. Older
/// brokers ignore the extra field and still decode the record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredTagRecord {
    #[serde(flatten)]
    pub(super) record: SnapshotTagRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) owner: Option<SnapshotTag>,
}
