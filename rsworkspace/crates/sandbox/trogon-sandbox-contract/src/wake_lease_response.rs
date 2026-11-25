use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationId};

/// `Woken.deadline` is the lease's run deadline after the wake.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum WakeLeaseResponse {
    Woken {
        deadline: SystemTime,
        /// The durable operation that carried the wake, when the broker recorded one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<OperationId>,
    },
    Denied(LeaseError),
}
