use serde::{Deserialize, Serialize};

use crate::{LeaseId, OperationId, TenantId};

/// Client-facing request to freeze a running lease in place. State (files and,
/// where the backend allows, processes) is kept and the lease id stays valid,
/// unlike a stop, which releases the lease. The lease deadline is replaced by
/// a paused budget so the lease is left alone until the budget runs out; a
/// resume restores the run time that was left. Idempotent: pausing a paused
/// lease succeeds with no change.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PauseLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    /// How long the lease may stay paused before it is reaped. The broker
    /// defaults it to 3600 seconds and clamps it to 7 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_paused_seconds: Option<u32>,
    /// Names this pause as a durable operation. Retrying with the same id
    /// replays the stored result; the broker picks one when it is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
