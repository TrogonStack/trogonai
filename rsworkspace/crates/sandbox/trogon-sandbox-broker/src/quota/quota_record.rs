use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::pending_attempt_quota::PendingAttemptQuota;
use super::prewarm_quota::PrewarmQuota;
use super::snapshot_quota::SnapshotQuota;
use crate::constants::{
    DEFAULT_ACTIVE_LEASE_CAP, DEFAULT_QUOTA_CPU_SECONDS, DEFAULT_QUOTA_WALL_SECONDS, MAX_REFUND_RECEIPT_BYTES,
    MAX_REFUND_RECEIPTS, REFUND_RECEIPT_OVERHEAD_BYTES, REFUND_RECEIPT_RETENTION,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRecord {
    pub cpu_seconds_used: u32,
    pub wall_seconds_used: u32,
    pub cap_cpu_seconds: u32,
    pub cap_wall_seconds: u32,
    #[serde(default)]
    pub snapshot: SnapshotQuota,
    #[serde(default)]
    pub active_leases: u32,
    #[serde(default = "default_active_lease_cap")]
    pub active_lease_cap: u32,
    #[serde(default)]
    pub prewarm: PrewarmQuota,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_attempt: Option<PendingAttemptQuota>,
    /// Releases already refunded to this record, by lease id, with the Unix
    /// second the refund was applied. Lives in the record so the receipt and
    /// the refund commit in one compare-and-swap: a refund repeated after a
    /// crash, a redelivery or a second broker finds its receipt and does
    /// nothing. Receipts older than [`REFUND_RECEIPT_RETENTION`] are dropped
    /// when the record is next written.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refunded_releases: BTreeMap<String, u64>,
}

fn default_active_lease_cap() -> u32 {
    DEFAULT_ACTIVE_LEASE_CAP
}

impl QuotaRecord {
    /// A record for a tenant seen for the first time through a counter other than CPU or wall time.
    pub(super) fn on_demand() -> Self {
        Self::new(DEFAULT_QUOTA_CPU_SECONDS, DEFAULT_QUOTA_WALL_SECONDS)
    }

    pub fn new(cap_cpu_seconds: u32, cap_wall_seconds: u32) -> Self {
        Self {
            cpu_seconds_used: 0,
            wall_seconds_used: 0,
            cap_cpu_seconds,
            cap_wall_seconds,
            snapshot: SnapshotQuota::default(),
            active_leases: 0,
            active_lease_cap: default_active_lease_cap(),
            prewarm: PrewarmQuota::default(),
            pending_attempt: None,
            refunded_releases: BTreeMap::new(),
        }
    }

    pub fn with_snapshot_quota(
        cap_cpu_seconds: u32,
        cap_wall_seconds: u32,
        snapshot: SnapshotQuota,
        active_lease_cap: u32,
    ) -> Self {
        Self {
            cpu_seconds_used: 0,
            wall_seconds_used: 0,
            cap_cpu_seconds,
            cap_wall_seconds,
            snapshot,
            active_leases: 0,
            active_lease_cap,
            prewarm: PrewarmQuota::default(),
            pending_attempt: None,
            refunded_releases: BTreeMap::new(),
        }
    }

    pub fn would_exceed(&self, cpu_seconds: u32, wall_seconds: u32) -> bool {
        self.cpu_seconds_used.saturating_add(cpu_seconds) > self.cap_cpu_seconds
            || self.wall_seconds_used.saturating_add(wall_seconds) > self.cap_wall_seconds
    }

    pub fn consume(&mut self, cpu_seconds: u32, wall_seconds: u32) {
        self.cpu_seconds_used = self.cpu_seconds_used.saturating_add(cpu_seconds);
        self.wall_seconds_used = self.wall_seconds_used.saturating_add(wall_seconds);
    }

    pub fn release(&mut self, cpu_seconds: u32, wall_seconds: u32) {
        self.cpu_seconds_used = self.cpu_seconds_used.saturating_sub(cpu_seconds);
        self.wall_seconds_used = self.wall_seconds_used.saturating_sub(wall_seconds);
    }

    /// Returns a released lease's quota and its active slot, once per
    /// `release_id`. False, with nothing changed, when this record already
    /// holds the receipt. `now` is Unix seconds.
    pub fn refund_release_once(&mut self, release_id: &str, now: u64, cpu_seconds: u32, wall_seconds: u32) -> bool {
        let keep = REFUND_RECEIPT_RETENTION.as_secs();
        self.refunded_releases.retain(|_, at| now.saturating_sub(*at) < keep);
        if self.refunded_releases.contains_key(release_id) {
            return false;
        }
        self.release(cpu_seconds, wall_seconds);
        self.decrement_active_leases();
        self.refunded_releases.insert(release_id.to_string(), now);
        let mut bytes: usize = self
            .refunded_releases
            .keys()
            .map(|id| id.len() + REFUND_RECEIPT_OVERHEAD_BYTES)
            .sum();
        while self.refunded_releases.len() > MAX_REFUND_RECEIPTS || bytes > MAX_REFUND_RECEIPT_BYTES {
            let Some(oldest) = self
                .refunded_releases
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            bytes -= oldest.len() + REFUND_RECEIPT_OVERHEAD_BYTES;
            self.refunded_releases.remove(&oldest);
        }
        true
    }

    pub fn active_lease_count(&self) -> u32 {
        self.active_leases
    }

    pub fn at_active_lease_cap(&self) -> bool {
        self.active_leases >= self.active_lease_cap
    }

    pub fn increment_active_leases(&mut self) {
        self.active_leases = self.active_leases.saturating_add(1);
    }

    pub fn decrement_active_leases(&mut self) {
        self.active_leases = self.active_leases.saturating_sub(1);
    }
}

pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
