use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxGroupName};

/// Broker-to-worker request to create a Linux group inside a running lease's sandbox. Only the
/// worker holding `lease_id` replies.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CreateSandboxGroupRequest {
    pub lease_id: LeaseId,
    pub groupname: SandboxGroupName,
}
