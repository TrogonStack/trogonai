use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of a [`DeleteSnapshotTagRequest`](crate::DeleteSnapshotTagRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DeleteSnapshotTagResponse {
    Applied,
    NotFound,
    Denied(LeaseError),
}
