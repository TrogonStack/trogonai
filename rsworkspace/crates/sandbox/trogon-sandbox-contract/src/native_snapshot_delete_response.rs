use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of a [`NativeSnapshotDeleteRequest`](crate::NativeSnapshotDeleteRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum NativeSnapshotDeleteResponse {
    Deleted,
    NotFound,
    Unsupported,
    Denied(LeaseError),
}
