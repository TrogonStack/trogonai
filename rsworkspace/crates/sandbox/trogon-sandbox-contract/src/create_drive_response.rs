use serde::{Deserialize, Serialize};

use crate::{DriveError, DriveRecord};

/// Outcome of a [`CreateDriveRequest`](crate::CreateDriveRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CreateDriveResponse {
    Created(DriveRecord),
    AlreadyExists(DriveRecord),
    Denied(DriveError),
}
