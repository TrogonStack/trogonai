use serde::{Deserialize, Serialize};

use crate::{DispatchAttemptId, FileSha256, LeaseEpoch, LeaseId, TenantId, WorkerId};

/// The signed envelope the broker adds to a dispatch so a worker can check where it came from.
///
/// It names the tenant, lease, operation, target worker and attempt, an expiry, and the SHA-256 of the rest of the
/// body. The signature is carried as text. Checking it is not part of this crate.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DispatchAuth {
    pub version: u8,
    pub key_id: String,
    pub operation: String,
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<DispatchAttemptId>,
    /// Unique per signed per-lease request, so a worker can refuse a replay. Lease dispatches carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    /// The lease epoch of the assignment, signed only when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LeaseEpoch>,
    /// Seconds since the Unix epoch after which the envelope is no longer valid.
    pub expires_at: u64,
    /// The hex SHA-256 of the dispatch JSON without this envelope.
    pub body_sha256: FileSha256,
    pub signature: String,
}
