use serde::{Deserialize, Serialize};

use crate::{LeaseError, SnapshotListRow};

/// Outcome of a [`SnapshotGetRequest`](crate::SnapshotGetRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SnapshotGetResponse {
    Found {
        row: SnapshotListRow,
    },
    NotFound,
    /// The caller may not read the tenant's snapshots. Kept apart from `NotFound` so a refusal is not mistaken for a
    /// missing digest.
    Denied(LeaseError),
}
