use serde::{Deserialize, Serialize};

use crate::TenantId;

/// Client request for a tenant's [`AccountLimits`](crate::AccountLimits).
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct GetAccountLimitsRequest {
    pub tenant: TenantId,
}
