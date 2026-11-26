use serde::{Deserialize, Serialize};

use crate::{LeaseError, SnapshotTreeNode};

/// The lineage nodes under a [`SnapshotTreeRequest`](crate::SnapshotTreeRequest).
///
/// `denied` is set when the caller may not read the tenant's snapshots, so a refusal is not mistaken for an empty
/// tree. Older brokers never send it, and it is omitted when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTreeResponse {
    pub nodes: Vec<SnapshotTreeNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied: Option<LeaseError>,
}
