use serde::{Deserialize, Serialize};

use crate::{LeaseFailedReason, LeaseId, TenantId};

/// Announces that a worker failed a lease, with the reason.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LeaseFailed {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub reason: LeaseFailedReason,
}
