use serde::{Deserialize, Serialize};

/// Where a committed snapshot generation stands in the tenant's storage accounting.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotAccountingDisposition {
    Live,
    Superseded,
    Deleted,
}
