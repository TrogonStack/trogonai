use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of a [`SnapshotDeleteRequest`](crate::SnapshotDeleteRequest), tagged by `outcome`.
///
/// `Unsupported` means the broker has no snapshot registry access.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SnapshotDeleteResponse {
    Deleted { bytes: u64 },
    NotFound,
    Unsupported,
    Denied(LeaseError),
}
