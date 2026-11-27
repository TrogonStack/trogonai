use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseId, SnapshotGeneration, TenantId};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureReservation {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    pub capture_key: String,
    pub generation: SnapshotGeneration,
    pub sequence: u64,
    pub created_at: SystemTime,
}
