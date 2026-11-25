use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a wake request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SandboxWakeResponse {
    Done,
    Unsupported,
    Denied(LeaseError),
}
