use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseError};

/// A worker's answer to a [`RestartCommandRequest`](crate::RestartCommandRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RestartCommandResponse {
    /// The command runs again as `execution_id`.
    Restarted {
        execution_id: ExecutionId,
    },
    NotFound,
    Unsupported,
    Denied(LeaseError),
}
