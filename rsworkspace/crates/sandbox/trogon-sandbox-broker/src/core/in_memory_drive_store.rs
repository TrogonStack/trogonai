use super::lock::lock;
use super::*;

/// Revision and write time of one drive's last claim change.
#[derive(Clone, Copy)]
pub(crate) struct ClaimMark {
    revision: u64,
    at: SystemTime,
}

/// In-memory [`DriveStore`] test double, mirroring the style of the other
/// `InMemory*` store test doubles used by `BrokerCore` (e.g.
/// `InMemoryActiveLeaseStore`, `InMemorySnapshotTagStore`). Lives here
/// (rather than in `drive_store.rs`) so both this crate's unit tests and its
/// external `tests/*.rs` integration tests can construct a `BrokerCore`
/// without a real NATS connection.
#[derive(Default)]
pub struct InMemoryDriveStore {
    drives: std::sync::Mutex<
        std::collections::HashMap<trogon_sandbox_contract::DriveId, trogon_sandbox_contract::DriveRecord>,
    >,
    /// Revision and write time of each drive's last claim change.
    claim_meta: std::sync::Mutex<std::collections::HashMap<trogon_sandbox_contract::DriveId, ClaimMark>>,
}

impl InMemoryDriveStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn bump(&self, drive_id: &trogon_sandbox_contract::DriveId) {
        let mut meta = lock(&self.claim_meta);
        let revision = meta.get(drive_id).map_or(0, |mark| mark.revision);
        meta.insert(
            drive_id.clone(),
            ClaimMark {
                revision: revision + 1,
                at: SystemTime::now(),
            },
        );
    }

    /// Pretends the drive's claim was written `age` ago.
    #[cfg(test)]
    pub(crate) fn backdate_claim(&self, drive_id: &trogon_sandbox_contract::DriveId, age: Duration) {
        let mut meta = lock(&self.claim_meta);
        let entry = meta.entry(drive_id.clone()).or_insert(ClaimMark {
            revision: 1,
            at: SystemTime::now(),
        });
        entry.at = SystemTime::now() - age;
    }

    /// Moves the drive to a new revision, as a concurrent re-claim would.
    #[cfg(test)]
    pub(crate) fn touch_revision(&self, drive_id: &trogon_sandbox_contract::DriveId) {
        self.bump(drive_id);
    }
}

#[async_trait::async_trait]
impl DriveStore for InMemoryDriveStore {
    async fn create_provider_volume(
        &self,
        tenant: &trogon_sandbox_contract::TenantId,
        drive_id: &trogon_sandbox_contract::DriveId,
        size_gib: u64,
        volume: trogon_sandbox_contract::ProviderVolumeRef,
    ) -> Result<trogon_sandbox_contract::DriveRecord, DriveStoreError> {
        let mut drives = lock(&self.drives);
        if drives.contains_key(drive_id) {
            return Err(DriveStoreError::AlreadyExists);
        }
        let record = trogon_sandbox_contract::DriveRecord {
            id: drive_id.clone(),
            tenant: tenant.clone(),
            size_gib,
            created_at: SystemTime::now(),
            attached_lease: None,
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: volume,
            },
        };
        drives.insert(drive_id.clone(), record.clone());
        Ok(record)
    }
    async fn create(
        &self,
        tenant: &trogon_sandbox_contract::TenantId,
        drive_id: &trogon_sandbox_contract::DriveId,
        size_gib: u64,
        bucket: String,
    ) -> Result<trogon_sandbox_contract::DriveRecord, DriveStoreError> {
        let mut drives = lock(&self.drives);
        if drives.contains_key(drive_id) {
            return Err(DriveStoreError::AlreadyExists);
        }
        let record = trogon_sandbox_contract::DriveRecord {
            id: drive_id.clone(),
            tenant: tenant.clone(),
            size_gib,
            created_at: std::time::SystemTime::now(),
            attached_lease: None,
            storage: trogon_sandbox_contract::DriveStorage::S3 { bucket },
        };
        drives.insert(drive_id.clone(), record.clone());
        Ok(record)
    }

    async fn get(
        &self,
        drive_id: &trogon_sandbox_contract::DriveId,
    ) -> Result<Option<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
        Ok(lock(&self.drives).get(drive_id).cloned())
    }

    async fn list(
        &self,
        tenant: &trogon_sandbox_contract::TenantId,
    ) -> Result<Vec<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
        Ok(lock(&self.drives)
            .values()
            .filter(|r| &r.tenant == tenant)
            .cloned()
            .collect())
    }

    async fn delete(&self, drive_id: &trogon_sandbox_contract::DriveId) -> Result<(), DriveStoreError> {
        let mut drives = lock(&self.drives);
        if drives.remove(drive_id).is_none() {
            return Err(DriveStoreError::NotFound);
        }
        Ok(())
    }

    async fn set_attached_lease(
        &self,
        drive_id: &trogon_sandbox_contract::DriveId,
        lease_id: Option<LeaseId>,
    ) -> Result<(), DriveStoreError> {
        let mut drives = lock(&self.drives);
        let record = drives.get_mut(drive_id).ok_or(DriveStoreError::NotFound)?;
        if let Some(new_lease) = &lease_id
            && let Some(other_lease) = &record.attached_lease
            && other_lease != new_lease
        {
            return Err(DriveStoreError::AlreadyAttached);
        }
        record.attached_lease = lease_id;
        drop(drives);
        self.bump(drive_id);
        Ok(())
    }

    async fn release_if_attached(
        &self,
        drive_id: &trogon_sandbox_contract::DriveId,
        lease_id: &LeaseId,
    ) -> Result<bool, DriveStoreError> {
        let mut drives = lock(&self.drives);
        let Some(record) = drives.get_mut(drive_id) else {
            return Err(DriveStoreError::NotFound);
        };
        if record.attached_lease.as_ref() != Some(lease_id) {
            return Ok(false);
        }
        record.attached_lease = None;
        drop(drives);
        self.bump(drive_id);
        Ok(true)
    }

    async fn list_claims(&self, after: Option<&str>, limit: usize) -> Result<ClaimPage, DriveStoreError> {
        let drives = lock(&self.drives);
        let meta = lock(&self.claim_meta);
        let mut ordered: Vec<(String, &trogon_sandbox_contract::DriveRecord)> = drives
            .values()
            .map(|record| (crate::drive_store::drive_kv_key(&record.id), record))
            .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
            .collect();
        ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let limit = limit.max(1);
        let next = (ordered.len() > limit).then(|| ordered[limit - 1].0.clone());
        ordered.truncate(limit);
        let claims = ordered
            .into_iter()
            .filter_map(|(_, record)| {
                let lease_id = record.attached_lease.clone()?;
                let (revision, at) = meta
                    .get(&record.id)
                    .map(|mark| (mark.revision, mark.at))
                    .unwrap_or((0, record.created_at));
                Some(DriveClaim {
                    drive_id: record.id.clone(),
                    lease_id,
                    revision,
                    claimed_at: Some(at),
                })
            })
            .collect();
        Ok(ClaimPage { claims, next })
    }

    async fn release_claim_at(
        &self,
        drive_id: &trogon_sandbox_contract::DriveId,
        lease_id: &LeaseId,
        revision: u64,
    ) -> Result<bool, DriveStoreError> {
        let current = lock(&self.claim_meta).get(drive_id).map_or(0, |mark| mark.revision);
        if current != revision {
            return Ok(false);
        }
        self.release_if_attached(drive_id, lease_id).await.or_else(|e| match e {
            DriveStoreError::NotFound => Ok(false),
            e => Err(e),
        })
    }
}
