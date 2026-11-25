use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseError};

/// A worker's answer to a [`CancelCommandLaunchRequest`](crate::CancelCommandLaunchRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CancelCommandLaunchResponse {
    Canceled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
    },
    Preserved {
        execution_id: ExecutionId,
    },
    Pending {
        message: String,
    },
    Unsupported,
    Denied(LeaseError),
}
