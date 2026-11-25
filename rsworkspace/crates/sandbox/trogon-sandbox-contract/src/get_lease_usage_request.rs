use serde::{Deserialize, Serialize};

use crate::{LeaseId, TenantId};

/// Client request for one lease's [`LeaseUsage`](crate::LeaseUsage).
///
/// A lease owned by another tenant reads the same as one the broker holds no figures for.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct GetLeaseUsageRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
}
