use serde::{Deserialize, Serialize};

use crate::{LeaseError, SnapshotTagAuditEntry};

/// The entries answering a [`SnapshotTagHistoryRequest`](crate::SnapshotTagHistoryRequest).
///
/// `denied` is set when the caller may not read the tenant's tags, so a refusal is not mistaken for an empty history.
/// Older brokers never send it, and it is omitted when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTagHistoryResponse {
    pub entries: Vec<SnapshotTagAuditEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied: Option<LeaseError>,
}
