use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseId, SandboxId, TenantId};

/// Outcome of a [`CancelOwnedLeaseRequest`](crate::CancelOwnedLeaseRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CancelOwnedLeaseResponse {
    Cleaned {
        tenant: TenantId,
        lease_id: LeaseId,
        sandbox_id: SandboxId,
    },
    NotHeld,
    Denied(LeaseError),
}
