use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`WriteFilesRequest`](crate::WriteFilesRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum WriteFilesResponse {
    Written,
    Denied(LeaseError),
}
