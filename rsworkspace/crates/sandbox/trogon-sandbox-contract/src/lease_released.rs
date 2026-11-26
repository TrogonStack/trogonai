use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{CorrelationId, Digest, LeaseId, LeaseReleasedReason, SnapshotPolicy, TenantId, WorkerId};

/// Announces that a lease ended, with the reason.
///
/// The optional fields are omitted from JSON when absent and default to `None` when an older peer
/// leaves them out. `released_at` uses serde's `SystemTime` encoding, an object of
/// `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseReleased {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub reason: LeaseReleasedReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_out: Option<SnapshotPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    /// Digest of the snapshot the lease was restored from; it becomes the parent of the snapshot
    /// exported on release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<Digest>,
    /// The release time, retained across cleanup and event-delivery retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<SystemTime>,
}
