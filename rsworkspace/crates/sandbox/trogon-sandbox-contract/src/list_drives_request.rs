use serde::{Deserialize, Serialize};

use crate::TenantId;

/// Client-facing request to list the drives of a tenant.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListDrivesRequest {
    pub tenant: TenantId,
}
