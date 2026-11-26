use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`ReadFileRequest`](crate::ReadFileRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ReadFileResponse {
    Content { data: Vec<u8> },
    NotFound,
    Denied(LeaseError),
}
