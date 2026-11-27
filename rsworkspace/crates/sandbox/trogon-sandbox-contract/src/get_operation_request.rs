use serde::{Deserialize, Serialize};

use crate::{OperationId, TenantId};

/// A tenant-scoped lookup of one operation. Another tenant's operation id reads as not found.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetOperationRequest {
    pub tenant: TenantId,
    pub operation_id: OperationId,
}
