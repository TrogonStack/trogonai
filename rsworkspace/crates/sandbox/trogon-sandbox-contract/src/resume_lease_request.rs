use serde::{Deserialize, Serialize};

use crate::{LeaseId, OperationId, TenantId};

/// Client-facing request to thaw a paused lease. Idempotent: resuming a
/// running lease succeeds with no change.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResumeLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    /// Names this resume as a durable operation. Retrying with the same id
    /// replays the stored result; the broker picks one when it is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
