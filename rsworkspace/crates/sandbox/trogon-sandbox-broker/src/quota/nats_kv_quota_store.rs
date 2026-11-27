use std::sync::Arc;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::time::SystemTime;
use trogon_sandbox_contract::{AttemptQuotaReservation, LeaseError, LeaseId, TenantId};
use trogon_sandbox_protocol::parse_tenant_scoped_kv_key;

use super::attempt_budget::month_of;
use super::attempt_quota_limits::AttemptQuotaLimits;
use super::attempt_quota_receipt::AttemptQuotaPhase;
use super::quota_keys::{legacy_quota_kv_key_for_month, nats_kv_keys, quota_kv_key_for_month};
use super::quota_record::{QuotaRecord, unix_seconds};
use super::quota_store::QuotaStore;
use super::quota_store_error::QuotaStoreError;
use super::snapshot_quota::SnapshotQuota;
use crate::constants::{LEGACY_SNAPSHOT_INVENTORY_BOUND, MAX_CAS_RETRIES, QUOTA_RETENTION};
use crate::snapshot_accounting::SnapshotAccountingStore;

mod attempts;

pub struct NatsKvQuotaStore {
    pub kv: kv::Store,
    snapshots: Arc<SnapshotAccountingStore>,
    snapshot_live: Arc<crate::snapshot_live::NatsKvLiveSnapshotStore>,
    snapshot_defaults: SnapshotQuota,
    cas_retries: u8,
    settlement_cas_retries: u8,
    legacy_inventory_bound: usize,
}

impl NatsKvQuotaStore {
    pub async fn new(jetstream: jetstream::Context, bucket: &str) -> Result<Self, QuotaStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: bucket.to_string(),
                history: 1,
                max_age: QUOTA_RETENTION,
                ..Default::default()
            })
            .await?;
        let snapshot_live = crate::snapshot_live::NatsKvLiveSnapshotStore::open(jetstream.clone())
            .await
            .map_err(QuotaStoreError::snapshot_store)?;
        let snapshots = SnapshotAccountingStore::open(jetstream)
            .await
            .map_err(QuotaStoreError::snapshot_store)?;
        Ok(Self {
            kv,
            snapshots,
            snapshot_live,
            snapshot_defaults: SnapshotQuota::default(),
            cas_retries: MAX_CAS_RETRIES,
            settlement_cas_retries: MAX_CAS_RETRIES,
            legacy_inventory_bound: LEGACY_SNAPSHOT_INVENTORY_BOUND,
        })
    }

    pub fn with_snapshot_defaults(mut self, defaults: SnapshotQuota) -> Self {
        self.snapshot_defaults = defaults;
        self
    }

    /// Compare-and-swap attempts one mutation makes before it gives up.
    pub fn with_cas_retries(mut self, retries: u8) -> Self {
        self.cas_retries = retries;
        self
    }

    /// Compare-and-swap attempts settling a pending attempt makes before it gives up.
    pub fn with_settlement_cas_retries(mut self, retries: u8) -> Self {
        self.settlement_cas_retries = retries;
        self
    }

    /// Most keys the legacy snapshot inventory scan visits before it refuses.
    pub fn with_legacy_inventory_bound(mut self, bound: usize) -> Self {
        self.legacy_inventory_bound = bound;
        self
    }

    /// Reads the quota record at `key`. When the current key has never been
    /// written and the legacy key holds a record, the current key is created
    /// once with the legacy record's counters, so the month's usage carries
    /// over without being counted twice: every later change applies to the
    /// current key only and the legacy record is left untouched.
    async fn entry_seeded(&self, key: &str, legacy: &str) -> Result<Option<kv::Entry>, QuotaStoreError> {
        if let Some(entry) = self.kv.entry(key).await? {
            return Ok(Some(entry));
        }
        if legacy == key {
            return Ok(None);
        }
        match self.kv.entry(legacy).await? {
            Some(old) if old.operation == kv::Operation::Put => {
                match self.kv.create(key, old.value.clone()).await {
                    Ok(_) => {}
                    Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
                Ok(self.kv.entry(key).await?)
            }
            _ => Ok(None),
        }
    }

    async fn mutate_record<F>(&self, tenant: &TenantId, f: F) -> Result<(), QuotaStoreError>
    where
        F: FnMut(&mut QuotaRecord),
    {
        let (key, legacy) = nats_kv_keys(tenant);
        self.mutate_record_at(&key, &legacy, f).await
    }

    async fn mutate_record_at<F>(&self, key: &str, legacy: &str, mut f: F) -> Result<(), QuotaStoreError>
    where
        F: FnMut(&mut QuotaRecord),
    {
        let (key, legacy) = (key.to_string(), legacy.to_string());

        for _ in 0..self.cas_retries {
            match self.entry_seeded(&key, &legacy).await {
                Err(e) => return Err(e),
                Ok(None) => {
                    let mut record = QuotaRecord::on_demand();
                    f(&mut record);
                    let payload = Bytes::from(serde_json::to_vec(&record)?);
                    match self.kv.create(&key, payload).await {
                        Ok(_) => return Ok(()),
                        Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(Some(entry)) => {
                    let mut record: QuotaRecord = serde_json::from_slice(&entry.value)?;
                    f(&mut record);
                    let payload = Bytes::from(serde_json::to_vec(&record)?);
                    match self.kv.update(&key, payload, entry.revision).await {
                        Ok(_) => return Ok(()),
                        Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }

        Err(QuotaStoreError::CasRetriesExhausted)
    }

    pub(super) async fn read_record(&self, tenant: &TenantId) -> Result<Option<QuotaRecord>, QuotaStoreError> {
        let (key, legacy) = nats_kv_keys(tenant);
        match self.entry_seeded(&key, &legacy).await? {
            None => Ok(None),
            Some(entry) => Ok(Some(serde_json::from_slice(&entry.value)?)),
        }
    }
}

#[async_trait]
impl QuotaStore for NatsKvQuotaStore {
    fn snapshot_live_store(&self) -> Option<Arc<dyn crate::snapshot_live::LiveSnapshotStore>> {
        Some(self.snapshot_live.clone())
    }

    fn snapshot_defaults(&self) -> SnapshotQuota {
        self.snapshot_defaults.clone()
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
        let current = self.read_record(tenant).await?.map(|record| record.snapshot);
        if let Some(quota) = current
            .as_ref()
            .filter(|quota| quota.count_used != 0 || quota.bytes_used != 0)
        {
            self.snapshots
                .quarantine_legacy(tenant, quota)
                .await
                .map_err(QuotaStoreError::LegacyQuarantine)?;
            return Ok(current);
        }
        let legacy_prefix = format!("{}_", tenant.as_str().replace(':', "_"));
        let mut keys = self.kv.keys().await?;
        let mut scanned = 0usize;
        while let Some(key) = keys.next().await {
            let key = key?;
            scanned += 1;
            if scanned > self.legacy_inventory_bound {
                return Err(QuotaStoreError::LegacyInventoryTooLarge);
            }
            let name = parse_tenant_scoped_kv_key(&key, tenant)
                .or_else(|| key.strip_prefix(&legacy_prefix).map(str::to_string));
            let Some(name) = name else {
                continue;
            };
            if name.len() != 7 || name.as_bytes()[4] != b'-' {
                continue;
            }
            if let Some(entry) = self.kv.entry(&key).await?
                && entry.operation == kv::Operation::Put
            {
                let record: QuotaRecord = serde_json::from_slice(&entry.value)?;
                if record.snapshot.count_used != 0 || record.snapshot.bytes_used != 0 {
                    self.snapshots
                        .quarantine_legacy(tenant, &record.snapshot)
                        .await
                        .map_err(QuotaStoreError::LegacyQuarantine)?;
                    return Ok(Some(record.snapshot));
                }
            }
        }
        Ok(current)
    }
    async fn reserve_attempt(
        &self,
        reservation: &AttemptQuotaReservation,
        limits: &AttemptQuotaLimits,
    ) -> Result<(), LeaseError> {
        self.transition_attempt(reservation, limits, AttemptQuotaPhase::Held)
            .await
            .map(|_| ())
    }

    async fn release_attempt(
        &self,
        reservation: &AttemptQuotaReservation,
        limits: &AttemptQuotaLimits,
    ) -> Result<bool, QuotaStoreError> {
        self.transition_attempt(reservation, limits, AttemptQuotaPhase::Released)
            .await
            .map_err(QuotaStoreError::from)
    }
    async fn check_and_reserve(
        &self,
        tenant: &TenantId,
        cpu_seconds: u32,
        ttl_seconds: u32,
        default_cpu_cap: u32,
        default_wall_cap: u32,
    ) -> Result<(), LeaseError> {
        let (key, legacy) = nats_kv_keys(tenant);

        for _ in 0..self.cas_retries {
            match self.entry_seeded(&key, &legacy).await {
                Err(e) => {
                    return Err(LeaseError::Backend {
                        message: format!("quota store read error: {e}"),
                    });
                }
                Ok(None) => {
                    let mut record = QuotaRecord::new(default_cpu_cap, default_wall_cap);
                    if record.would_exceed(cpu_seconds, ttl_seconds) {
                        return Err(LeaseError::QuotaExceeded { tenant: tenant.clone() });
                    }
                    record.consume(cpu_seconds, ttl_seconds);
                    let payload = serde_json::to_vec(&record).map_err(|e| LeaseError::Backend {
                        message: format!("serialize error: {e}"),
                    })?;
                    match self.kv.create(&key, Bytes::from(payload)).await {
                        Ok(_) => return Ok(()),
                        Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                        Err(e) => {
                            return Err(LeaseError::Backend {
                                message: format!("quota store write error: {e}"),
                            });
                        }
                    }
                }
                Ok(Some(entry)) => {
                    let mut record: QuotaRecord =
                        serde_json::from_slice(&entry.value).map_err(|e| LeaseError::Backend {
                            message: format!("deserialize error: {e}"),
                        })?;
                    if record.would_exceed(cpu_seconds, ttl_seconds) {
                        return Err(LeaseError::QuotaExceeded { tenant: tenant.clone() });
                    }
                    record.consume(cpu_seconds, ttl_seconds);
                    let payload = serde_json::to_vec(&record).map_err(|e| LeaseError::Backend {
                        message: format!("serialize error: {e}"),
                    })?;
                    match self.kv.update(&key, Bytes::from(payload), entry.revision).await {
                        Ok(_) => return Ok(()),
                        Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                        Err(e) => {
                            return Err(LeaseError::Backend {
                                message: format!("quota store write error: {e}"),
                            });
                        }
                    }
                }
            }
        }

        Err(LeaseError::Backend {
            message: "quota store CAS failed after max retries".to_string(),
        })
    }

    async fn release(&self, tenant: &TenantId, cpu_seconds: u32, ttl_seconds: u32) -> Result<(), QuotaStoreError> {
        let (key, legacy) = nats_kv_keys(tenant);

        for _ in 0..self.cas_retries {
            let Some(entry) = self.entry_seeded(&key, &legacy).await? else {
                return Ok(());
            };

            let mut record: QuotaRecord = serde_json::from_slice(&entry.value)?;
            record.release(cpu_seconds, ttl_seconds);
            let payload = Bytes::from(serde_json::to_vec(&record)?);

            match self.kv.update(&key, payload, entry.revision).await {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(e) => return Err(e.into()),
            }
        }

        Err(QuotaStoreError::ReleaseCasRetriesExhausted)
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
        let key = quota_kv_key_for_month(tenant, year, month);
        let legacy = legacy_quota_kv_key_for_month(tenant, year, month);
        let mut applied = false;
        self.mutate_record_at(&key, &legacy, |record| {
            applied = record.refund_release_once(release_id.as_str(), unix_seconds(), cpu_seconds, wall_seconds);
        })
        .await?;
        Ok(applied)
    }

    async fn quota_record(&self, tenant: &TenantId) -> Result<Option<QuotaRecord>, QuotaStoreError> {
        let current = self.read_record(tenant).await?;
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
        Ok(self
            .read_record(tenant)
            .await?
            .map(|r| r.active_lease_count())
            .unwrap_or(0))
    }

    async fn check_active_lease_cap(&self, tenant: &TenantId) -> Result<(), LeaseError> {
        match self.read_record(tenant).await {
            Ok(Some(record)) if record.at_active_lease_cap() => Err(LeaseError::TooManyActiveLeases {
                tenant: tenant.clone(),
                cap: record.active_lease_cap,
            }),
            Ok(_) => Ok(()),
            Err(e) => Err(LeaseError::Backend { message: e.to_string() }),
        }
    }

    async fn increment_active_leases(&self, tenant: &TenantId) -> Result<(), QuotaStoreError> {
        self.mutate_record(tenant, |record| {
            record.increment_active_leases();
        })
        .await
    }

    async fn decrement_active_leases(&self, tenant: &TenantId) -> Result<(), QuotaStoreError> {
        self.mutate_record(tenant, |record| {
            record.decrement_active_leases();
        })
        .await
    }

    async fn record_snapshot_published(
        &self,
        tenant: &TenantId,
        bytes: u64,
        default_snapshot: SnapshotQuota,
    ) -> Result<(), LeaseError> {
        let (key, legacy) = nats_kv_keys(tenant);
        let defaults = default_snapshot;

        for _ in 0..self.cas_retries {
            match self.entry_seeded(&key, &legacy).await {
                Err(e) => {
                    return Err(LeaseError::Backend {
                        message: format!("quota store read error: {e}"),
                    });
                }
                Ok(None) => {
                    let mut record = QuotaRecord::on_demand();
                    record.snapshot = defaults.clone();
                    if record.snapshot.would_exceed_publish(bytes) {
                        return Err(LeaseError::SnapshotQuotaExceeded { tenant: tenant.clone() });
                    }
                    record.snapshot.record_publish(bytes);
                    let payload = serde_json::to_vec(&record).map_err(|e| LeaseError::Backend {
                        message: format!("serialize error: {e}"),
                    })?;
                    match self.kv.create(&key, Bytes::from(payload)).await {
                        Ok(_) => return Ok(()),
                        Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                        Err(e) => {
                            return Err(LeaseError::Backend {
                                message: format!("quota store write error: {e}"),
                            });
                        }
                    }
                }
                Ok(Some(entry)) => {
                    let mut record: QuotaRecord =
                        serde_json::from_slice(&entry.value).map_err(|e| LeaseError::Backend {
                            message: format!("deserialize error: {e}"),
                        })?;
                    if record.snapshot.would_exceed_publish(bytes) {
                        return Err(LeaseError::SnapshotQuotaExceeded { tenant: tenant.clone() });
                    }
                    record.snapshot.record_publish(bytes);
                    let payload = serde_json::to_vec(&record).map_err(|e| LeaseError::Backend {
                        message: format!("serialize error: {e}"),
                    })?;
                    match self.kv.update(&key, Bytes::from(payload), entry.revision).await {
                        Ok(_) => return Ok(()),
                        Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                        Err(e) => {
                            return Err(LeaseError::Backend {
                                message: format!("quota store write error: {e}"),
                            });
                        }
                    }
                }
            }
        }

        Err(LeaseError::Backend {
            message: "quota store CAS failed after max retries".to_string(),
        })
    }

    async fn record_snapshot_deleted(&self, tenant: &TenantId, bytes: u64) -> Result<(), QuotaStoreError> {
        self.mutate_record(tenant, |record| {
            record.snapshot.record_delete(bytes);
        })
        .await
    }

    async fn check_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), LeaseError> {
        match self.read_record(tenant).await {
            Ok(Some(record)) if record.prewarm.would_exceed_dispatch(cpu_seconds) => {
                Err(LeaseError::PrewarmQuotaExceeded { tenant: tenant.clone() })
            }
            Ok(_) => Ok(()),
            Err(e) => Err(LeaseError::Backend { message: e.to_string() }),
        }
    }

    async fn record_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), QuotaStoreError> {
        self.mutate_record(tenant, |record| {
            record.prewarm.record_dispatch(cpu_seconds);
        })
        .await
    }

    async fn record_prewarm_release(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), QuotaStoreError> {
        self.mutate_record(tenant, |record| {
            record.prewarm.record_release(cpu_seconds);
        })
        .await
    }
}
