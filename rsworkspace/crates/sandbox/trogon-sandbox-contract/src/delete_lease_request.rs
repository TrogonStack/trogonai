use serde::{Deserialize, Serialize};

use crate::{LeaseId, OperationId, TenantId};

/// Client-facing request to permanently delete a lease.
///
/// It stops the sandbox like a cancel and additionally purges any resumable persistence tied to the lease: the
/// snapshot tag and suspended actor of a named lease, and attached drives. A cancel, by contrast, preserves anything
/// tagged for later resume.
///
/// `operation_id` names this delete as a durable operation. Retrying with the same id replays the stored result, and
/// the broker picks one when it is unset. It is omitted when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeleteLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
