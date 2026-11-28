use super::*;
use crate::active_lease::{ActiveLeaseStore, ActiveLeaseStoreError};
use crate::drive_store::DriveStoreError;
use std::time::SystemTime;
use trogon_sandbox_contract::{DriveError, DriveRecord};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn unavailable() -> ActiveLeaseStoreError {
    ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable"))
}

struct BrokenLeases;

#[async_trait::async_trait]
impl ActiveLeaseStore for BrokenLeases {
    async fn put(&self, _: &LeaseRecord) -> Result<(), ActiveLeaseStoreError> {
        Err(unavailable())
    }
    async fn get(&self, _: &LeaseId) -> Result<Option<LeaseRecord>, ActiveLeaseStoreError> {
        Err(unavailable())
    }
    async fn delete(&self, _: &LeaseId) -> Result<bool, ActiveLeaseStoreError> {
        Err(unavailable())
    }
    async fn all(&self) -> Result<Vec<LeaseRecord>, ActiveLeaseStoreError> {
        Err(unavailable())
    }
    async fn touch(&self, _: &LeaseId, _: SystemTime) -> Result<bool, ActiveLeaseStoreError> {
        Err(unavailable())
    }
    async fn replace_if_unchanged(&self, _: &LeaseRecord, _: &LeaseRecord) -> Result<bool, ActiveLeaseStoreError> {
        Err(unavailable())
    }
}

struct BrokenDrives;

#[async_trait::async_trait]
impl DriveStore for BrokenDrives {
    async fn create(&self, _: &TenantId, _: &DriveId, _: u64, _: String) -> Result<DriveRecord, DriveStoreError> {
        Err(DriveStoreError::Backend("kv unavailable".into()))
    }
    async fn get(&self, _: &DriveId) -> Result<Option<DriveRecord>, DriveStoreError> {
        Err(DriveStoreError::Backend("kv unavailable".into()))
    }
    async fn list(&self, _: &TenantId) -> Result<Vec<DriveRecord>, DriveStoreError> {
        Err(DriveStoreError::Backend("kv unavailable".into()))
    }
    async fn delete(&self, _: &DriveId) -> Result<(), DriveStoreError> {
        Err(DriveStoreError::Backend("kv unavailable".into()))
    }
    async fn set_attached_lease(&self, _: &DriveId, _: Option<LeaseId>) -> Result<(), DriveStoreError> {
        Err(DriveStoreError::Backend("kv unavailable".into()))
    }
    async fn release_if_attached(&self, _: &DriveId, _: &LeaseId) -> Result<bool, DriveStoreError> {
        Err(DriveStoreError::Backend("kv unavailable".into()))
    }
}

#[tokio::test]
async fn listing_leases_with_an_unreadable_store_is_denied_not_empty() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    core.active_leases = Arc::new(BrokenLeases);

    let response = core
        .handle_list_leases(ListLeasesRequest {
            tenant: TenantId::new("test-tenant")?,
            status_filter: None,
            cursor: None,
            limit: None,
            tags: Default::default(),
        })
        .await;

    assert!(matches!(response.denied, Some(LeaseError::Backend { .. })));
    assert!(response.leases.is_empty());
    Ok(())
}

#[tokio::test]
async fn listing_drives_with_an_unreadable_store_is_denied_not_empty() -> TestResult {
    let core = make_core_with_drive_store(mock_provider_response()?, Arc::new(BrokenDrives))?;

    let response = core
        .handle_list_drives(ListDrivesRequest {
            tenant: TenantId::new("test-tenant")?,
        })
        .await;

    assert!(matches!(response.denied, Some(DriveError::StorageBackend { .. })));
    assert!(response.drives.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_dispatch_that_cannot_be_signed_is_refused_not_sent_unsigned() -> TestResult {
    let signer = Arc::new(trogon_sandbox_protocol::DispatchSigner::new("k1", [9u8; 32]));
    let core = make_core(mock_provider_response()?)?.with_dispatch_signer(signer);

    assert!(core.sign_dispatch(Bytes::from_static(b"not json")).is_err());
    let unsigned_core = make_core(mock_provider_response()?)?;
    assert_eq!(
        unsigned_core.sign_dispatch(Bytes::from_static(b"not json"))?,
        Bytes::from_static(b"not json")
    );
    Ok(())
}
