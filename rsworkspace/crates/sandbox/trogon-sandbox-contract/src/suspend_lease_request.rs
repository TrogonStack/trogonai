use serde::{Deserialize, Serialize};

use crate::{LeaseId, OperationId, SuspendRequestLevel, TenantId};

/// Client-facing request to suspend a lease: its host resources are released
/// and a durable artifact is kept, so a wake brings the same lease id back.
/// Unlike a pause, which freezes in place, a suspend survives the worker
/// holding the sandbox going away. The run deadline is parked and replaced by
/// a suspended budget, as for a pause. A suspend that cannot flush the sandbox
/// fails and leaves the lease as it was. Idempotent: suspending a suspended
/// lease succeeds with no change.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SuspendLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    /// How deep to suspend. Defaults to `auto`, the level the provider offers.
    #[serde(default)]
    pub level: SuspendRequestLevel,
    /// How long the lease may stay suspended before it is released and its
    /// artifact deleted. The broker defaults it to 24 hours and clamps it to 7 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_suspended_seconds: Option<u32>,
    /// Names this suspend as a durable operation. Retrying with the same id
    /// replays the stored result; the broker picks one when it is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
