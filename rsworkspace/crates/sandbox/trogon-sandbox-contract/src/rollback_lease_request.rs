use serde::{Deserialize, Serialize};

use crate::{LeaseId, OciArtifactRef, OperationId, TenantId};

/// Client-facing request to reset a running lease to a snapshot while keeping
/// its id, deadline and tenant. The sandbox is rebuilt from `snapshot`, so
/// whatever was in it is gone, and the lease's processes restart empty. The
/// snapshot must be one the tenant owns. Idempotent under an operation id.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RollbackLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    pub snapshot: OciArtifactRef,
    /// Names this rollback as a durable operation. Retrying with the same id
    /// replays the stored result; the broker picks one when it is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
