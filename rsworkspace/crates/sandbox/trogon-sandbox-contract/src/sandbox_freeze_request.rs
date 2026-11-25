use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// Broker-to-worker request to freeze (or, on the resume subject, thaw) the
/// sandbox of a lease. Broadcast to every worker for the lease's provider; only
/// the worker holding `lease_id` replies. Workers must treat a repeat as
/// success.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxFreezeRequest {
    pub lease_id: LeaseId,
}
