use serde::{Deserialize, Serialize};

use crate::DriveError;

/// Outcome of a [`DeleteDriveRequest`](crate::DeleteDriveRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DeleteDriveResponse {
    Applied,
    Denied(DriveError),
}
