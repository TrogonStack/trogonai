use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseCounters, LeaseId, LeaseRecord, LeaseStatus, LeaseUsage, TenantId};

/// What the store keeps per lease.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub created_at: SystemTime,
    pub last_activity_at: SystemTime,
    #[serde(default)]
    pub counters: LeaseCounters,
    /// Set once the broker releases the lease. Counting stops from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<SystemTime>,
}

impl UsageEntry {
    pub fn new(lease: &LeaseRecord) -> Self {
        Self {
            lease_id: lease.lease_id.clone(),
            tenant: lease.tenant.clone(),
            created_at: lease.created_at,
            last_activity_at: lease.created_at,
            counters: LeaseCounters::default(),
            ended_at: None,
        }
    }

    /// Folds a pause still in progress into `paused_seconds` as of `at`.
    pub(super) fn close_pause(&mut self, at: SystemTime) {
        if let Some(paused_at) = self.counters.paused_at.take() {
            let spent = at.duration_since(paused_at).map_or(0, |d| d.as_secs());
            self.counters.paused_seconds = self.counters.paused_seconds.saturating_add(spent);
        }
    }

    /// Folds a suspend still in progress into `suspended_seconds` as of `at`.
    pub(super) fn close_suspend(&mut self, at: SystemTime) {
        if let Some(suspended_at) = self.counters.suspended_at.take() {
            let spent = at.duration_since(suspended_at).map_or(0, |d| d.as_secs());
            self.counters.suspended_seconds = self.counters.suspended_seconds.saturating_add(spent);
        }
        self.counters.suspend_artifact_bytes = None;
    }

    /// The figures as of `now`. `status` is the lease's live status, or
    /// `Released` once the entry has ended.
    pub fn to_usage(&self, status: LeaseStatus, now: SystemTime) -> LeaseUsage {
        let until = self.ended_at.unwrap_or(now);
        let in_progress = self
            .counters
            .paused_at
            .map_or(0, |at| until.duration_since(at).map_or(0, |d| d.as_secs()));
        let paused_seconds = self.counters.paused_seconds.saturating_add(in_progress);
        let suspended_in_progress = self
            .counters
            .suspended_at
            .map_or(0, |at| until.duration_since(at).map_or(0, |d| d.as_secs()));
        let suspended_seconds = self.counters.suspended_seconds.saturating_add(suspended_in_progress);
        let elapsed = until.duration_since(self.created_at).map_or(0, |d| d.as_secs());
        LeaseUsage {
            lease_id: self.lease_id.clone(),
            tenant: self.tenant.clone(),
            status,
            created_at: self.created_at,
            ended_at: self.ended_at,
            last_activity_at: self.last_activity_at,
            commands_run: self.counters.commands_run,
            command_output_bytes: self.counters.command_output_bytes,
            file_bytes_in: self.counters.file_bytes_in,
            file_bytes_out: self.counters.file_bytes_out,
            running_seconds: elapsed.saturating_sub(paused_seconds).saturating_sub(suspended_seconds),
            paused_seconds,
            suspended_seconds,
            suspend_artifact_bytes: self.counters.suspend_artifact_bytes.filter(|bytes| *bytes > 0),
            gauges: None,
        }
    }
}
