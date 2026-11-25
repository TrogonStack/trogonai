use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{DispatchAttemptId, LeaseAccounting, LeaseEpoch, LeaseReleasedReason, LeaseRequestKey, PendingDispatch};

/// One dispatch attempt of a lease.
///
/// The optional fields are omitted from JSON when absent and default to `None` when an older
/// record leaves them out. `cleanup_started_at` uses serde's `SystemTime` encoding, an object of
/// `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseDispatchAttempt {
    pub id: DispatchAttemptId,
    /// The epoch stamped on this attempt's dispatch. `None` when the worker did not advertise
    /// epoch support, so nothing about it is sent or signed for that worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LeaseEpoch>,
    pub idempotency_key: LeaseRequestKey,
    pub accounting: LeaseAccounting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_reason: Option<LeaseReleasedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_started_at: Option<SystemTime>,
    /// The exact dispatch sent to the worker, journaled while the attempt is pending so a broker
    /// can re-send it when the reply was lost. Cleared once the attempt commits as running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_dispatch: Option<PendingDispatch>,
}
