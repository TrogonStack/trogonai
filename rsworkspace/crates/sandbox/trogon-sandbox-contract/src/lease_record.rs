use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    AttemptQuotaReservation, CorrelationId, Digest, DormancyRecord, FreezeInFlight, IdlePauseRetry,
    LeaseDispatchAttempt, LeaseEpoch, LeaseId, LeaseName, LeaseStatus, ProviderKind, SandboxEntityId, SandboxSpec,
    SandboxTags, TenantId, WorkerId,
};

/// The broker's durable record of one lease.
///
/// Optional fields are omitted from JSON when absent and default when an older record leaves them out. Times use
/// serde's `SystemTime` encoding, an object of `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub provider: ProviderKind,
    pub status: LeaseStatus,
    pub created_at: SystemTime,
    pub deadline: SystemTime,
    pub spec: SandboxSpec,
    /// The provider's opaque sandbox identifier. It is an empty string for records written before the field
    /// existed, which is why it stays plain text.
    #[serde(default)]
    pub sandbox_id: String,
    #[serde(default)]
    pub cpu_seconds_consumed: u32,
    #[serde(default)]
    pub wall_seconds_consumed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(default)]
    pub tags: SandboxTags,
    /// The name the lease was requested with, kept so its name index entry can be cleared on release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<LeaseName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_attempt: Option<LeaseDispatchAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_reservation: Option<AttemptQuotaReservation>,
    /// The order of this lease's worker assignments. Absent for records written before epochs existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LeaseEpoch>,
    /// The last time the broker routed a per-lease verb for this lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<SystemTime>,
    /// Seconds of run time left when the lease was paused. Absent unless the lease is paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_remaining_seconds: Option<u64>,
    /// A pause or resume the broker started but has not yet recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeze_in_flight: Option<FreezeInFlight>,
    /// When the idle evaluator paused the lease. Absent for a lease paused by request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_paused_at: Option<SystemTime>,
    /// The backoff state of idle pauses that did not take effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_pause_retry: Option<IdlePauseRetry>,
    /// The digest of the snapshot this lease was restored from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<Digest>,
    /// The persistent entity this lease is a run of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<SandboxEntityId>,
    /// Set when a suspend starts and kept after the wake. Absent for a lease that was never suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dormancy: Option<DormancyRecord>,
}
