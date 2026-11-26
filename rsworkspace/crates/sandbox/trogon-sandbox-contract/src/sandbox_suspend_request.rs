use serde::{Deserialize, Serialize};

use crate::{LeaseId, SuspendLevel};

/// Broker-to-worker request to suspend the sandbox of a lease. Broadcast to
/// every worker for the lease's provider; only the worker holding `lease_id`
/// replies. Workers must treat a repeat of the same `generation` as success.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxSuspendRequest {
    pub lease_id: LeaseId,
    pub generation: u64,
    pub level: SuspendLevel,
}
