use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{DispatchAttemptId, LeaseAccounting, TenantId};

/// The quota held for a dispatch attempt while it runs.
///
/// `created_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttemptQuotaReservation {
    pub tenant: TenantId,
    pub attempt_id: DispatchAttemptId,
    pub created_at: SystemTime,
    pub cpu_seconds: u32,
    pub wall_seconds: u32,
    pub accounting: LeaseAccounting,
}
