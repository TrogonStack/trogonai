use serde::{Deserialize, Serialize};

use crate::TenantId;

/// Client request for a tenant's [`TenantUsage`](crate::TenantUsage).
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct GetTenantUsageRequest {
    pub tenant: TenantId,
}
