use async_trait::async_trait;
use trogon_sandbox_contract::{DriveId, DriveRecord, LeaseId, TenantId};

use super::claim_page::ClaimPage;
use super::drive_store_error::DriveStoreError;

#[async_trait]
pub trait DriveStore: Send + Sync {
    async fn create(
        &self,
        tenant: &TenantId,
        drive_id: &DriveId,
        size_gib: u64,
        bucket: String,
    ) -> Result<DriveRecord, DriveStoreError>;

    async fn create_provider_volume(
        &self,
        _tenant: &TenantId,
        _drive_id: &DriveId,
        _size_gib: u64,
        _volume: trogon_sandbox_contract::ProviderVolumeRef,
    ) -> Result<DriveRecord, DriveStoreError> {
        Err(DriveStoreError::Backend(
            "native provider volume registration is unavailable".into(),
        ))
    }

    async fn get(&self, drive_id: &DriveId) -> Result<Option<DriveRecord>, DriveStoreError>;

    async fn list(&self, tenant: &TenantId) -> Result<Vec<DriveRecord>, DriveStoreError>;

    async fn delete(&self, drive_id: &DriveId) -> Result<(), DriveStoreError>;

    async fn set_attached_lease(&self, drive_id: &DriveId, lease_id: Option<LeaseId>) -> Result<(), DriveStoreError>;

    /// Clears `attached_lease` only when `lease_id` holds the drive. Returns
    /// `Ok(false)` when another lease (or none) holds it and `NotFound` when
    /// no drive with this id exists, including one owned by another tenant.
    async fn release_if_attached(&self, _drive_id: &DriveId, _lease_id: &LeaseId) -> Result<bool, DriveStoreError> {
        Err(DriveStoreError::Backend(
            "exact-owner drive release is unavailable".into(),
        ))
    }

    /// Examines at most `limit` drives in a stable order, starting after the
    /// position `after` returned by an earlier page (from the beginning when
    /// `None`), and reports those that carry a claim. Paging keeps one scan
    /// bounded while successive pages cover every drive. Stores that cannot
    /// enumerate claims report none, which disables the orphan sweep.
    async fn list_claims(&self, _after: Option<&str>, _limit: usize) -> Result<ClaimPage, DriveStoreError> {
        Ok(ClaimPage::default())
    }

    /// Clears the claim `lease_id` holds on `drive_id` only if the drive is
    /// still at `revision`. `Ok(false)` means the drive changed, was released
    /// or is gone, and nothing was written.
    async fn release_claim_at(
        &self,
        _drive_id: &DriveId,
        _lease_id: &LeaseId,
        _revision: u64,
    ) -> Result<bool, DriveStoreError> {
        Ok(false)
    }

    /// [`Self::release_if_attached`] for cleanup paths, where a drive that no
    /// longer exists holds no claim to release.
    async fn release_claim(&self, drive_id: &DriveId, lease_id: &LeaseId) -> Result<bool, DriveStoreError> {
        match self.release_if_attached(drive_id, lease_id).await {
            Err(DriveStoreError::NotFound) => Ok(false),
            result => result,
        }
    }
}
