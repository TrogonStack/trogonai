use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseCounters, LeaseId, LeaseRecord, TenantId};

use super::{UsageEntry, UsageStoreError};

#[async_trait]
pub trait LeaseUsageStore: Send + Sync {
    /// Applies `change` to the lease's entry, creating it from `lease` first
    /// when absent. Implementations make the read-modify-write atomic across
    /// replicas.
    async fn modify(
        &self,
        lease: &LeaseRecord,
        change: &(dyn for<'e> Fn(&'e mut UsageEntry) + Send + Sync),
    ) -> Result<(), UsageStoreError>;

    /// The entry for `lease_id`, or `None` when it belongs to another tenant.
    async fn get(&self, tenant: &TenantId, lease_id: &LeaseId) -> Result<Option<UsageEntry>, UsageStoreError>;

    /// Every entry the store still keeps for `tenant`, live or ended.
    async fn tenant_entries(&self, tenant: &TenantId) -> Result<Vec<UsageEntry>, UsageStoreError>;

    /// Adds `delta` and stamps the lease's last activity. Ignored once the
    /// lease has ended.
    async fn add(&self, lease: &LeaseRecord, delta: &LeaseCounters, at: SystemTime) -> Result<(), UsageStoreError> {
        self.modify(lease, &|entry| {
            if entry.ended_at.is_some() {
                return;
            }
            entry.counters.add(delta);
            entry.last_activity_at = entry.last_activity_at.max(at);
        })
        .await
    }

    /// Starts counting paused time. A repeat keeps the first start.
    async fn mark_paused(&self, lease: &LeaseRecord, at: SystemTime) -> Result<(), UsageStoreError> {
        self.modify(lease, &|entry| {
            if entry.ended_at.is_none() && entry.counters.paused_at.is_none() {
                entry.counters.paused_at = Some(at);
            }
        })
        .await
    }

    /// Stops counting paused time. A repeat is a no-op.
    async fn mark_resumed(&self, lease: &LeaseRecord, at: SystemTime) -> Result<(), UsageStoreError> {
        self.modify(lease, &|entry| {
            if entry.ended_at.is_none() {
                entry.close_pause(at);
            }
        })
        .await
    }

    /// Starts counting suspended time and records the artifact size. A repeat
    /// keeps the first start. A pause still counting is closed first, so the
    /// two never overlap.
    async fn mark_suspended(
        &self,
        lease: &LeaseRecord,
        at: SystemTime,
        artifact_bytes: Option<u64>,
    ) -> Result<(), UsageStoreError> {
        self.modify(lease, &|entry| {
            if entry.ended_at.is_some() {
                return;
            }
            entry.close_pause(at);
            if entry.counters.suspended_at.is_none() {
                entry.counters.suspended_at = Some(at);
            }
            entry.counters.suspend_artifact_bytes = artifact_bytes;
        })
        .await
    }

    /// Stops counting suspended time. A repeat is a no-op.
    async fn mark_woken(&self, lease: &LeaseRecord, at: SystemTime) -> Result<(), UsageStoreError> {
        self.modify(lease, &|entry| {
            if entry.ended_at.is_none() {
                entry.close_suspend(at);
            }
        })
        .await
    }

    /// Freezes the entry as the lease's final figures. A repeat keeps the
    /// first end time.
    async fn finish(&self, lease: &LeaseRecord, at: SystemTime) -> Result<(), UsageStoreError> {
        self.modify(lease, &|entry| {
            if entry.ended_at.is_none() {
                entry.close_pause(at);
                entry.close_suspend(at);
                entry.ended_at = Some(at);
            }
        })
        .await
    }
}
