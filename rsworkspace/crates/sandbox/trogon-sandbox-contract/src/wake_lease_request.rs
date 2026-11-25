use serde::{Deserialize, Serialize};

use crate::{LeaseId, OperationId, TenantId};

/// Client-facing request to wake a suspended lease. Idempotent: waking a
/// running lease succeeds with no change.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WakeLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    /// Names this wake as a durable operation. Retrying with the same id
    /// replays the stored result; the broker picks one when it is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
