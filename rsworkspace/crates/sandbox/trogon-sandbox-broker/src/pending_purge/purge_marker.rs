use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{DriveId, LeaseId, LeaseReleased, TenantId};

use super::PendingRefund;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingPurge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reservation: Option<trogon_sandbox_contract::AttemptQuotaReservation>,
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Revision of the snapshot tag under `name` when the delete started.
    /// Only that revision is removed, so a tag set under the name afterwards
    /// survives the purge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_revision: Option<u64>,
    /// Whether the release this purge started was announced with a
    /// `lease_released` event. `Some(false)` until the release finished, so
    /// the reconciler can announce a release that stopped after removing the
    /// active record. `None` on markers written before this field existed,
    /// which are never re-announced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_announced: Option<bool>,
    /// The quota the purge's release still owes the tenant. The release
    /// removes the active record before it refunds, so a broker that stops in
    /// between would lose the refund; until the release returns, the
    /// reconciler redoes it from here once the active record is gone. The
    /// refund itself is idempotent per lease id in the quota store, so this
    /// field only says a redo may be needed; clearing it late costs nothing.
    /// `None` once settled, for releases that refund through their dispatch
    /// attempt, and on markers written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_refund: Option<PendingRefund>,
    /// The drives the lease mounted when the delete started, so the claims
    /// the release clears after removing the active record can be cleared
    /// again from the marker alone. The released lease record is not
    /// guaranteed to be readable afterwards. `None` on markers written before
    /// this field existed, which fall back to the released record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_claims: Option<Vec<DriveId>>,
    /// The `lease_released` event the release publishes, so a release that
    /// stopped before publishing it can be announced from the marker alone.
    /// `None` on markers written before this field existed, which are never
    /// re-announced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcement: Option<LeaseReleased>,
    pub recorded_at: SystemTime,
}
