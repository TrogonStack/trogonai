use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseId, LeaseStatus, ResourceGauges, TenantId};

/// What the broker reports for one lease.
///
/// A released lease keeps reporting its final figures, with `status` set to `released` and
/// `ended_at` set, until the broker's usage store expires them.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LeaseUsage {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub status: LeaseStatus,
    pub created_at: SystemTime,
    /// When the broker released the lease. `None` while it is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<SystemTime>,
    /// Last time the broker routed a counted verb for the lease, or `created_at` when it has not.
    pub last_activity_at: SystemTime,
    pub commands_run: u64,
    pub command_output_bytes: u64,
    pub file_bytes_in: u64,
    pub file_bytes_out: u64,
    /// Wall-clock seconds since creation, excluding time spent paused or suspended.
    pub running_seconds: u64,
    /// Wall-clock seconds spent paused, including a pause still in progress.
    pub paused_seconds: u64,
    /// Wall-clock seconds spent suspended, including a suspend still in progress.
    #[serde(default)]
    pub suspended_seconds: u64,
    /// Bytes the lease's suspend artifact takes now. Absent unless suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend_artifact_bytes: Option<u64>,
    /// Present only when the owning provider reports gauges and answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gauges: Option<ResourceGauges>,
}
