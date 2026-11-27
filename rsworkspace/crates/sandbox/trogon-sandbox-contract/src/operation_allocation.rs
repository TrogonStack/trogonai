use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{AttemptQuotaReservation, DispatchTarget, LeaseId, ProviderKind, SandboxId};

/// The sandbox a create operation allocated, kept so a cancellation can clean it up.
///
/// `created_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationAllocation {
    pub lease_id: LeaseId,
    pub provider: ProviderKind,
    pub sandbox_id: SandboxId,
    pub created_at: SystemTime,
    pub reservation: AttemptQuotaReservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<DispatchTarget>,
}
