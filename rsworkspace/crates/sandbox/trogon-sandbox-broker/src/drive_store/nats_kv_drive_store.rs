use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::kv::{self, Operation};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use trogon_sandbox_contract::{DriveId, DriveRecord, LeaseId, TenantId};
use trogon_sandbox_protocol::subjects::DRIVE_KV_BUCKET;
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_prefix, tenant_scoped_kv_prefix};

use super::claim_page::ClaimPage;
use super::drive_claim::DriveClaim;
use super::drive_keys::{drive_kv_key, legacy_drive_kv_key, owns};
use super::drive_store_error::DriveStoreError;
use super::store::DriveStore;
use crate::constants::{DRIVE_KV_HISTORY, MAX_CAS_RETRIES};

pub struct NatsKvDriveStore {
    pub kv: kv::Store,
}

impl NatsKvDriveStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, DriveStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: DRIVE_KV_BUCKET.to_string(),
                history: DRIVE_KV_HISTORY,
                max_age: Duration::from_secs(0),
                ..Default::default()
            })
            .await
            .map_err(DriveStoreError::backend)?;
        Ok(Self { kv })
    }

    async fn read_entry(&self, key: &str) -> Result<Option<(DriveRecord, u64)>, DriveStoreError> {
        match self.kv.entry(key).await {
            Ok(None) => Ok(None),
            Ok(Some(entry)) if matches!(entry.operation, Operation::Delete | Operation::Purge) => Ok(None),
            Ok(Some(entry)) => {
                let record: DriveRecord = serde_json::from_slice(&entry.value)
                    .map_err(|e| DriveStoreError::backend(format!("decode drive record: {e}")))?;
                Ok(Some((record, entry.revision)))
            }
            Err(e) => Err(DriveStoreError::backend(e)),
        }
    }

    /// Finds the key holding `drive_id`'s own record: the current key, else
    /// the legacy key when its record names this drive.
    async fn locate(&self, drive_id: &DriveId) -> Result<Option<(String, DriveRecord, u64)>, DriveStoreError> {
        let key = drive_kv_key(drive_id);
        if let Some((record, revision)) = self.read_entry(&key).await?
            && owns(&record, drive_id)
        {
            return Ok(Some((key, record, revision)));
        }
        let legacy = legacy_drive_kv_key(drive_id);
        if legacy != key
            && let Some((record, revision)) = self.read_entry(&legacy).await?
            && owns(&record, drive_id)
        {
            return Ok(Some((legacy, record, revision)));
        }
        Ok(None)
    }

    async fn insert(&self, record: DriveRecord) -> Result<DriveRecord, DriveStoreError> {
        let key = drive_kv_key(&record.id);
        let legacy = legacy_drive_kv_key(&record.id);
        if legacy != key
            && let Some((existing, _)) = self.read_entry(&legacy).await?
            && owns(&existing, &record.id)
        {
            return Err(DriveStoreError::AlreadyExists);
        }
        let payload = serde_json::to_vec(&record).map_err(DriveStoreError::backend)?;
        match self.kv.create(&key, Bytes::from(payload)).await {
            Ok(_) => Ok(record),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Err(DriveStoreError::AlreadyExists),
            Err(error) => Err(DriveStoreError::backend(error)),
        }
    }
}

#[async_trait]
impl DriveStore for NatsKvDriveStore {
    async fn create(
        &self,
        tenant: &TenantId,
        drive_id: &DriveId,
        size_gib: u64,
        bucket: String,
    ) -> Result<DriveRecord, DriveStoreError> {
        let record = DriveRecord {
            id: drive_id.clone(),
            tenant: tenant.clone(),
            size_gib,
            created_at: std::time::SystemTime::now(),
            attached_lease: None,
            storage: trogon_sandbox_contract::DriveStorage::S3 { bucket },
        };
        self.insert(record).await
    }

    async fn create_provider_volume(
        &self,
        tenant: &TenantId,
        drive_id: &DriveId,
        size_gib: u64,
        volume: trogon_sandbox_contract::ProviderVolumeRef,
    ) -> Result<DriveRecord, DriveStoreError> {
        self.insert(DriveRecord {
            id: drive_id.clone(),
            tenant: tenant.clone(),
            size_gib,
            created_at: std::time::SystemTime::now(),
            attached_lease: None,
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: volume,
            },
        })
        .await
    }

    async fn get(&self, drive_id: &DriveId) -> Result<Option<DriveRecord>, DriveStoreError> {
        Ok(self.locate(drive_id).await?.map(|(_, record, _)| record))
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<DriveRecord>, DriveStoreError> {
        let prefix = tenant_scoped_kv_prefix(tenant);
        let legacy_prefix = legacy_tenant_scoped_kv_prefix(tenant);
        let mut keys = self.kv.keys().await.map_err(DriveStoreError::backend)?;
        let mut out = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(DriveStoreError::backend)?;
            if !key.starts_with(&prefix) && !key.starts_with(&legacy_prefix) {
                continue;
            }
            if let Some((record, _)) = self.read_entry(&key).await?
                && record.tenant == *tenant
                && record.id.tenant == *tenant
            {
                out.push(record);
            }
        }
        out.sort_by(|a, b| a.id.name.cmp(&b.id.name));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    async fn delete(&self, drive_id: &DriveId) -> Result<(), DriveStoreError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some((key, _, revision)) = self.locate(drive_id).await? else {
                return Err(DriveStoreError::NotFound);
            };
            match self.kv.delete_expect_revision(&key, Some(revision)).await {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
                Err(e) => {
                    return Err(DriveStoreError::backend(format!("delete drive: {e}")));
                }
            }
        }
        Err(DriveStoreError::backend("drive store CAS failed after max retries"))
    }

    async fn set_attached_lease(&self, drive_id: &DriveId, lease_id: Option<LeaseId>) -> Result<(), DriveStoreError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some((key, mut record, revision)) = self.locate(drive_id).await? else {
                return Err(DriveStoreError::NotFound);
            };
            if let Some(new_lease) = &lease_id
                && let Some(other_lease) = &record.attached_lease
                && other_lease != new_lease
            {
                return Err(DriveStoreError::AlreadyAttached);
            }
            record.attached_lease = lease_id.clone();
            let payload = serde_json::to_vec(&record)
                .map_err(|e| DriveStoreError::backend(format!("encode drive record: {e}")))?;
            match self.kv.update(&key, Bytes::from(payload), revision).await {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(e) => {
                    return Err(DriveStoreError::backend(format!("update drive attached_lease: {e}")));
                }
            }
        }
        Err(DriveStoreError::backend("drive store CAS failed after max retries"))
    }

    async fn release_if_attached(&self, drive_id: &DriveId, lease_id: &LeaseId) -> Result<bool, DriveStoreError> {
        for _ in 0..MAX_CAS_RETRIES {
            let Some((key, mut record, revision)) = self.locate(drive_id).await? else {
                return Err(DriveStoreError::NotFound);
            };
            if record.attached_lease.as_ref() != Some(lease_id) {
                return Ok(false);
            }
            record.attached_lease = None;
            let payload = serde_json::to_vec(&record).map_err(DriveStoreError::backend)?;
            match self.kv.update(&key, payload.into(), revision).await {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(DriveStoreError::backend(error)),
            }
        }
        Err(DriveStoreError::backend(
            "exact-owner drive release exhausted CAS retries",
        ))
    }

    async fn list_claims(&self, after: Option<&str>, limit: usize) -> Result<ClaimPage, DriveStoreError> {
        let mut keys = self.kv.keys().await.map_err(DriveStoreError::backend)?;
        let limit = limit.max(1);
        // Keep only the smallest `limit + 1` keys past the cursor, so a page
        // holds a bounded set however many keys the bucket has. The extra key
        // says whether another page follows.
        let mut window = std::collections::BTreeSet::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(DriveStoreError::backend)?;
            if after.is_none_or(|after| key.as_str() > after) {
                window.insert(key);
                if window.len() > limit + 1 {
                    window.pop_last();
                }
            }
        }
        let mut all: Vec<String> = window.into_iter().collect();
        let next = (all.len() > limit).then(|| all[limit - 1].clone());
        all.truncate(limit);
        let mut out = Vec::new();
        for key in all {
            let entry = match self.kv.entry(&key).await {
                Ok(Some(entry)) if !matches!(entry.operation, Operation::Delete | Operation::Purge) => entry,
                Ok(_) => continue,
                Err(e) => return Err(DriveStoreError::backend(e)),
            };
            let Ok(record) = serde_json::from_slice::<DriveRecord>(&entry.value) else {
                continue;
            };
            let Some(lease_id) = record.attached_lease else {
                continue;
            };
            out.push(DriveClaim {
                drive_id: record.id,
                lease_id,
                revision: entry.revision,
                claimed_at: Some(std::time::SystemTime::from(entry.created)),
            });
        }
        Ok(ClaimPage { claims: out, next })
    }

    async fn release_claim_at(
        &self,
        drive_id: &DriveId,
        lease_id: &LeaseId,
        revision: u64,
    ) -> Result<bool, DriveStoreError> {
        let Some((key, mut record, current)) = self.locate(drive_id).await? else {
            return Ok(false);
        };
        if current != revision || record.attached_lease.as_ref() != Some(lease_id) {
            return Ok(false);
        }
        record.attached_lease = None;
        let payload = serde_json::to_vec(&record).map_err(DriveStoreError::backend)?;
        match self.kv.update(&key, payload.into(), revision).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(false),
            Err(error) => Err(DriveStoreError::backend(error)),
        }
    }
}
