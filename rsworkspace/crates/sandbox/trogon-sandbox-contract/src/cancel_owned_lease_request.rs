use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxId, TenantId};

/// Request to clean up the provider sandbox a lease owns.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CancelOwnedLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    pub sandbox_id: SandboxId,
}
