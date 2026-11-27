use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    AttemptQuotaReservation, LeaseError, LeaseId, SnapshotAccountingRefusal, SnapshotPublished, TenantId,
};

use super::attempt_budget::attempt_error;
use super::attempt_quota_limits::AttemptQuotaLimits;
use super::quota_record::QuotaRecord;
use super::quota_store_error::QuotaStoreError;
use super::snapshot_quota::SnapshotQuota;
use crate::snapshot_accounting::{ConfirmedPublication, SnapshotAccountingStore};

#[async_trait]
pub trait QuotaStore: Send + Sync {
    fn snapshot_live_store(&self) -> Option<Arc<dyn crate::snapshot_live::LiveSnapshotStore>> {
        None
    }

    fn snapshot_defaults(&self) -> SnapshotQuota {
        SnapshotQuota::default()
    }
    fn snapshot_store(&self) -> Option<Arc<SnapshotAccountingStore>> {
        None
    }

    async fn legacy_snapshot_quota(&self, _tenant: &TenantId) -> Result<Option<SnapshotQuota>, QuotaStoreError> {
        Ok(None)
    }

    async fn confirm_snapshot_publication(
        &self,
        event: &SnapshotPublished,
        defaults: SnapshotQuota,
    ) -> Result<ConfirmedPublication, SnapshotAccountingRefusal> {
        let store = self.snapshot_store().ok_or(SnapshotAccountingRefusal::Backend)?;
        let legacy = if store.usage(&event.tenant).await?.is_none() {
            self.legacy_snapshot_quota(&event.tenant)
                .await
                .map_err(|_| SnapshotAccountingRefusal::Backend)?
        } else {
            None
        };
        store.apply_publication(event, defaults, legacy).await
    }
    async fn reserve_attempt(
        &self,
        _reservation: &AttemptQuotaReservation,
        _limits: &AttemptQuotaLimits,
    ) -> Result<(), LeaseError> {
        Err(attempt_error("atomic dispatch accounting is unavailable"))
    }

    async fn release_attempt(
        &self,
        _reservation: &AttemptQuotaReservation,
        _limits: &AttemptQuotaLimits,
    ) -> Result<bool, QuotaStoreError> {
        Err(QuotaStoreError::AttemptAccountingUnavailable)
    }
    async fn check_and_reserve(
        &self,
        tenant: &TenantId,
        cpu_seconds: u32,
        ttl_seconds: u32,
        default_cpu_cap: u32,
        default_wall_cap: u32,
    ) -> Result<(), LeaseError>;

    async fn release(&self, tenant: &TenantId, cpu_seconds: u32, ttl_seconds: u32) -> Result<(), QuotaStoreError>;

    /// Returns the quota a released lease held, and its active slot, to the
    /// record of the month containing `month_of`, at most once per `release_id`
    /// however often it is called, from however many brokers. True when this
    /// call applied it. Callers pass the same `month_of` on every attempt so a
    /// repeat lands on the record that holds the receipt. The default does not
    /// remember receipts; stores shared between brokers must override it.
    async fn refund_release(
        &self,
        tenant: &TenantId,
        _release_id: &LeaseId,
        _month_of: SystemTime,
        cpu_seconds: u32,
        wall_seconds: u32,
    ) -> Result<bool, QuotaStoreError> {
        self.release(tenant, cpu_seconds, wall_seconds).await?;
        self.decrement_active_leases(tenant).await?;
        Ok(true)
    }

    /// The tenant's quota record for the current month, if one exists. Stores
    /// that cannot report it return `None`, which readers treat as untouched.
    async fn quota_record(&self, _tenant: &TenantId) -> Result<Option<QuotaRecord>, QuotaStoreError> {
        Ok(None)
    }

    async fn active_lease_count(&self, tenant: &TenantId) -> Result<u32, QuotaStoreError>;

    async fn check_active_lease_cap(&self, tenant: &TenantId) -> Result<(), LeaseError>;

    async fn increment_active_leases(&self, tenant: &TenantId) -> Result<(), QuotaStoreError>;

    async fn decrement_active_leases(&self, tenant: &TenantId) -> Result<(), QuotaStoreError>;

    async fn record_snapshot_published(
        &self,
        tenant: &TenantId,
        bytes: u64,
        default_snapshot: SnapshotQuota,
    ) -> Result<(), LeaseError>;

    async fn record_snapshot_deleted(&self, tenant: &TenantId, bytes: u64) -> Result<(), QuotaStoreError>;

    async fn check_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), LeaseError>;

    async fn record_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), QuotaStoreError>;

    async fn record_prewarm_release(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), QuotaStoreError>;
}
