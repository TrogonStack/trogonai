use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a freeze or thaw request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SandboxFreezeResponse {
    Done,
    Unsupported,
    Denied(LeaseError),
}
