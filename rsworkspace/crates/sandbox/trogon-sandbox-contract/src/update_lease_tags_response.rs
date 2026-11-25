use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of an [`UpdateLeaseTagsRequest`](crate::UpdateLeaseTagsRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum UpdateLeaseTagsResponse {
    Applied,
    Denied(LeaseError),
}
