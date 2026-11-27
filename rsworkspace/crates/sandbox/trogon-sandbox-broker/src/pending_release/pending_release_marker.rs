use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{DriveId, LeaseId, LeaseReleased, TenantId};

use crate::pending_purge::PendingRefund;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingRelease {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reservation: Option<trogon_sandbox_contract::AttemptQuotaReservation>,
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    /// The lease's name, so a redo can clear the name index entry that still
    /// points at the released lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the release returns to the tenant's quota. The refund is applied
    /// once per lease id by the quota store, into the month of `recorded_at`,
    /// so every attempt lands on the record that holds the receipt.
    pub refund: PendingRefund,
    /// The drives the lease mounted, so the claims can be cleared again
    /// without the released lease record.
    pub drive_claims: Vec<DriveId>,
    /// The event to publish, so a release that stopped before announcing can
    /// still be announced.
    pub announcement: LeaseReleased,
    pub recorded_at: SystemTime,
}
