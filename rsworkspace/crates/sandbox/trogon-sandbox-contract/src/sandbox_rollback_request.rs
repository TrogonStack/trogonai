use serde::{Deserialize, Serialize};

use crate::{LeaseId, OciArtifactRef};

/// Broker-to-worker request to rebuild the sandbox of a lease from a snapshot.
/// Broadcast to every worker for the lease's provider; only the worker holding
/// `lease_id` replies.
///
/// The request carries no identity of its own. A retry of one rollback and a
/// later rollback to the same snapshot look the same here, so the broker tells
/// them apart through the operation id of the client's [`RollbackLeaseRequest`]
/// and replays a retried operation without sending it again. A worker must
/// rebuild from `snapshot` on every request it receives and never skip one as a
/// duplicate. Rebuilding from the same snapshot twice leaves the same sandbox,
/// so a request the broker resends after a lost reply is safe.
///
/// [`RollbackLeaseRequest`]: crate::RollbackLeaseRequest
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxRollbackRequest {
    pub lease_id: LeaseId,
    pub snapshot: OciArtifactRef,
}
