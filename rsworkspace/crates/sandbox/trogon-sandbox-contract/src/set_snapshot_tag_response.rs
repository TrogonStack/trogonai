use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of a [`SetSnapshotTagRequest`](crate::SetSnapshotTagRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SetSnapshotTagResponse {
    Applied,
    Denied(LeaseError),
}
