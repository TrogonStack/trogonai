use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationId};

/// `Paused.reap_at` is when the broker reaps the lease if nobody resumes it.
/// `Unsupported` means the lease's provider cannot pause.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum PauseLeaseResponse {
    Paused {
        reap_at: SystemTime,
        /// The durable operation that carried the pause, when the broker recorded one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<OperationId>,
    },
    Unsupported,
    Denied(LeaseError),
}
