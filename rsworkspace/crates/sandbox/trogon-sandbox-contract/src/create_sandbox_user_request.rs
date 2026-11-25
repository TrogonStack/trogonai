use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxUserName};

/// Broker-to-worker request to create a Linux user inside a running lease's sandbox, so several
/// agents can work under separate accounts. Only the worker holding `lease_id` replies.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CreateSandboxUserRequest {
    pub lease_id: LeaseId,
    pub username: SandboxUserName,
}
