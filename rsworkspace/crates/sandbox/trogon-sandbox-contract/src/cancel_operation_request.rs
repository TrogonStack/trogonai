use serde::{Deserialize, Serialize};

use crate::{CreatedLeaseCompensation, OperationId, TenantId};

/// A request to cancel a durable operation.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CancelOperationRequest {
    pub tenant: TenantId,
    pub operation_id: OperationId,
    /// Whether to also clean up a lease the operation already created. Defaults to keeping it.
    #[serde(default)]
    pub compensate_created: CreatedLeaseCompensation,
}
