use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{AttemptQuotaReservation, DispatchAttemptId, LeaseError, LeaseId, TenantId};

use super::attempt_budget::{
    attempt_error, attempt_month_key, month_of, new_attempt_record, release_attempt_budget, reserve_attempt_budget,
    validate_unseen_attempt,
};
use super::attempt_quota_limits::AttemptQuotaLimits;
use super::attempt_quota_receipt::{AttemptQuotaPhase, AttemptQuotaReceipt};
use super::prewarm_quota::PrewarmQuota;
use super::quota_keys::quota_key;
use super::quota_record::{QuotaRecord, unix_seconds};
use super::quota_store::QuotaStore;
use super::quota_store_error::QuotaStoreError;
use super::snapshot_quota::SnapshotQuota;
use crate::snapshot_accounting::SnapshotAccountingStore;

pub struct InMemoryQuotaStore {
    pub(super) records: tokio::sync::Mutex<HashMap<String, QuotaRecord>>,
    pub(super) attempts: tokio::sync::Mutex<HashMap<DispatchAttemptId, AttemptQuotaReceipt>>,
    snapshots: Arc<SnapshotAccountingStore>,
    snapshot_live: Arc<crate::snapshot_live::InMemoryLiveSnapshotStore>,
    snapshot_defaults: SnapshotQuota,
}

impl InMemoryQuotaStore {
    pub fn with_snapshot_defaults(mut self, defaults: SnapshotQuota) -> Self {
        self.snapshot_defaults = defaults;
        self
    }
    pub fn new() -> Self {
        Self {
            records: tokio::sync::Mutex::new(HashMap::new()),
            attempts: tokio::sync::Mutex::new(HashMap::new()),
            snapshots: SnapshotAccountingStore::memory(),
            snapshot_live: crate::snapshot_live::InMemoryLiveSnapshotStore::new(),
            snapshot_defaults: SnapshotQuota::default(),
        }
    }

    #[cfg(test)]
    pub async fn seed_record(&self, tenant: &TenantId, record: QuotaRecord) {
        self.records.lock().await.insert(quota_key(tenant), record);
    }

    pub async fn seed_prewarm_quota(&self, tenant: &TenantId, prewarm: PrewarmQuota) {
        self.records.lock().await.insert(
            quota_key(tenant),
            QuotaRecord {
                prewarm,
                ..QuotaRecord::on_demand()
            },
        );
    }
}

impl Default for InMemoryQuotaStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaStore for InMemoryQuotaStore {
    fn snapshot_defaults(&self) -> SnapshotQuota {
        self.snapshot_defaults.clone()
    }
    fn snapshot_live_store(&self) -> Option<Arc<dyn crate::snapshot_live::LiveSnapshotStore>> {
        Some(self.snapshot_live.clone())
    }
    fn snapshot_store(&self) -> Option<Arc<SnapshotAccountingStore>> {
        Some(self.snapshots.clone())
    }

    async fn legacy_snapshot_quota(&self, tenant: &TenantId) -> Result<Option<SnapshotQuota>, QuotaStoreError> {
        if let Some(observed) = self
            .snapshots
            .legacy_observation(tenant)
            .await
            .map_err(QuotaStoreError::LegacyObservation)?
        {
            return Ok(Some(observed));
        }
        let observed = {
            let records = self.records.lock().await;
            records
                .iter()
                .find(|(key, record)| {
                    key.rsplit_once(':').is_some_and(|(owner, _)| owner == tenant.as_str())
                        && (record.snapshot.count_used != 0 || record.snapshot.bytes_used != 0)
                })
                .map(|(_, record)| record.snapshot.clone())
                .or_else(|| records.get(&quota_key(tenant)).map(|record| record.snapshot.clone()))
        };
        if let Some(observed) = &observed {
            self.snapshots
                .quarantine_legacy(tenant, observed)
                .await
                .map_err(QuotaStoreError::LegacyQuarantine)?;
        }
        Ok(observed)
    }
    async fn reserve_attempt(
        &self,
        reservation: &AttemptQuotaReservation,
        limits: &AttemptQuotaLimits,
    ) -> Result<(), LeaseError> {
        let key = attempt_month_key(reservation)?;
        let mut records = self.records.lock().await;
        let mut attempts = self.attempts.lock().await;
        if let Some(receipt) = attempts.get(&reservation.attempt_id) {
            if receipt.reservation != *reservation {
                return Err(attempt_error("quota attempt identity was reused"));
            }
            return if receipt.phase == AttemptQuotaPhase::Held {
                Ok(())
            } else {
                Err(attempt_error("quota attempt was released before admission"))
            };
        }
        validate_unseen_attempt(reservation, AttemptQuotaPhase::Held, limits)?;
        let record = records.entry(key).or_insert_with(|| new_attempt_record(limits));
        reserve_attempt_budget(record, reservation)?;
        attempts.insert(
            reservation.attempt_id.clone(),
            AttemptQuotaReceipt {
                reservation: reservation.clone(),
                phase: AttemptQuotaPhase::Held,
            },
        );
        Ok(())
    }

    async fn release_attempt(
        &self,
        reservation: &AttemptQuotaReservation,
        limits: &AttemptQuotaLimits,
    ) -> Result<bool, QuotaStoreError> {
        let key = attempt_month_key(reservation)?;
        let mut records = self.records.lock().await;
        let mut attempts = self.attempts.lock().await;
        let held = if let Some(receipt) = attempts.get(&reservation.attempt_id) {
            if receipt.reservation != *reservation {
                return Err(QuotaStoreError::AttemptIdentityReused);
            }
            if receipt.phase == AttemptQuotaPhase::Released {
                return Ok(false);
            }
            true
        } else {
            validate_unseen_attempt(reservation, AttemptQuotaPhase::Released, limits)?;
            false
        };
        if held {
            release_attempt_budget(
                records.get_mut(&key).ok_or(QuotaStoreError::HeldRecordMissing)?,
                reservation,
            )?;
        }
        attempts.insert(
            reservation.attempt_id.clone(),
            AttemptQuotaReceipt {
                reservation: reservation.clone(),
                phase: AttemptQuotaPhase::Released,
            },
        );
        Ok(held)
    }
    async fn check_and_reserve(
        &self,
        tenant: &TenantId,
        cpu_seconds: u32,
        ttl_seconds: u32,
        default_cpu_cap: u32,
        default_wall_cap: u32,
    ) -> Result<(), LeaseError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        let record = records
            .entry(key)
            .or_insert_with(|| QuotaRecord::new(default_cpu_cap, default_wall_cap));

        if record.would_exceed(cpu_seconds, ttl_seconds) {
            return Err(LeaseError::QuotaExceeded { tenant: tenant.clone() });
        }
        record.consume(cpu_seconds, ttl_seconds);
        Ok(())
    }

    async fn release(&self, tenant: &TenantId, cpu_seconds: u32, ttl_seconds: u32) -> Result<(), QuotaStoreError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        if let Some(record) = records.get_mut(&key) {
            record.release(cpu_seconds, ttl_seconds);
        }
        Ok(())
    }

    async fn refund_release(
        &self,
        tenant: &TenantId,
        release_id: &LeaseId,
        month_of_refund: SystemTime,
        cpu_seconds: u32,
        wall_seconds: u32,
    ) -> Result<bool, QuotaStoreError> {
        let (year, month) = month_of(month_of_refund)?;
        let key = format!("{}:{year:04}-{month:02}", tenant.as_str());
        let mut records = self.records.lock().await;
        let record = records.entry(key).or_insert_with(QuotaRecord::on_demand);
        Ok(record.refund_release_once(release_id.as_str(), unix_seconds(), cpu_seconds, wall_seconds))
    }

    async fn quota_record(&self, tenant: &TenantId) -> Result<Option<QuotaRecord>, QuotaStoreError> {
        let current = self.records.lock().await.get(&quota_key(tenant)).cloned();
        if let Some(record) = current.as_ref()
            && (record.snapshot.count_used != 0 || record.snapshot.bytes_used != 0)
        {
            self.snapshots
                .quarantine_legacy(tenant, &record.snapshot)
                .await
                .map_err(QuotaStoreError::LegacyQuarantine)?;
        }
        match self
            .snapshots
            .usage(tenant)
            .await
            .map_err(QuotaStoreError::SnapshotAccounting)?
        {
            Some(snapshot) => {
                let mut record = current.unwrap_or_else(QuotaRecord::on_demand);
                record.snapshot = snapshot;
                Ok(Some(record))
            }
            None => {
                if self
                    .legacy_snapshot_quota(tenant)
                    .await?
                    .is_some_and(|quota| quota.count_used != 0 || quota.bytes_used != 0)
                {
                    return Err(QuotaStoreError::LegacyUsageRequiresReconciliation);
                }
                Ok(current)
            }
        }
    }

    async fn active_lease_count(&self, tenant: &TenantId) -> Result<u32, QuotaStoreError> {
        let key = quota_key(tenant);
        let records = self.records.lock().await;
        Ok(records.get(&key).map(|r| r.active_lease_count()).unwrap_or(0))
    }

    async fn check_active_lease_cap(&self, tenant: &TenantId) -> Result<(), LeaseError> {
        let key = quota_key(tenant);
        let records = self.records.lock().await;
        if let Some(record) = records.get(&key)
            && record.at_active_lease_cap()
        {
            return Err(LeaseError::TooManyActiveLeases {
                tenant: tenant.clone(),
                cap: record.active_lease_cap,
            });
        }
        Ok(())
    }

    async fn increment_active_leases(&self, tenant: &TenantId) -> Result<(), QuotaStoreError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        let record = records.entry(key).or_insert_with(QuotaRecord::on_demand);
        record.increment_active_leases();
        Ok(())
    }

    async fn decrement_active_leases(&self, tenant: &TenantId) -> Result<(), QuotaStoreError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        if let Some(record) = records.get_mut(&key) {
            record.decrement_active_leases();
        }
        Ok(())
    }

    async fn record_snapshot_published(
        &self,
        tenant: &TenantId,
        bytes: u64,
        default_snapshot: SnapshotQuota,
    ) -> Result<(), LeaseError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        let record = records.entry(key).or_insert_with(|| {
            let mut r = QuotaRecord::on_demand();
            r.snapshot = default_snapshot.clone();
            r
        });
        if record.snapshot.would_exceed_publish(bytes) {
            return Err(LeaseError::SnapshotQuotaExceeded { tenant: tenant.clone() });
        }
        record.snapshot.record_publish(bytes);
        Ok(())
    }

    async fn record_snapshot_deleted(&self, tenant: &TenantId, bytes: u64) -> Result<(), QuotaStoreError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        if let Some(record) = records.get_mut(&key) {
            record.snapshot.record_delete(bytes);
        }
        Ok(())
    }

    async fn check_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), LeaseError> {
        let key = quota_key(tenant);
        let records = self.records.lock().await;
        if let Some(record) = records.get(&key)
            && record.prewarm.would_exceed_dispatch(cpu_seconds)
        {
            return Err(LeaseError::PrewarmQuotaExceeded { tenant: tenant.clone() });
        }
        Ok(())
    }

    async fn record_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), QuotaStoreError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        let record = records.entry(key).or_insert_with(QuotaRecord::on_demand);
        record.prewarm.record_dispatch(cpu_seconds);
        Ok(())
    }

    async fn record_prewarm_release(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), QuotaStoreError> {
        let key = quota_key(tenant);
        let mut records = self.records.lock().await;
        if let Some(record) = records.get_mut(&key) {
            record.prewarm.record_release(cpu_seconds);
        }
        Ok(())
    }
}
