use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId};

/// A request to poll the status of a previously started detached command.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PollCommandRequest {
    pub lease_id: LeaseId,
    pub execution_id: ExecutionId,
}
