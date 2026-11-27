use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseId, LiveSnapshotEntry, OciArtifactRef, SnapshotGeneration, TenantId};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDeletionClaim {
    pub tenant: TenantId,
    pub reference: OciArtifactRef,
    pub generation: SnapshotGeneration,
    pub lease_id: LeaseId,
    pub claim_id: String,
    pub sequence: u64,
    pub entry: LiveSnapshotEntry,
    pub charged: bool,
    pub completed: bool,
    pub projection_revision: u64,
}
