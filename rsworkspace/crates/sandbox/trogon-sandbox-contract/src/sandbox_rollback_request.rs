use serde::{Deserialize, Serialize};

use crate::{LeaseId, OciArtifactRef};

/// Broker-to-worker request to rebuild the sandbox of a lease from a snapshot.
/// Broadcast to every worker for the lease's provider; only the worker holding
/// `lease_id` replies. Workers must treat a repeat as success.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxRollbackRequest {
    pub lease_id: LeaseId,
    pub snapshot: OciArtifactRef,
}
