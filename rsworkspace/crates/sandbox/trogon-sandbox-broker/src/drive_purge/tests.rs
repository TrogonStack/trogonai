use std::sync::Arc;

use futures::StreamExt;
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use trogon_sandbox_contract::{DriveId, DriveRecord, DriveStorage, TenantId};

use super::*;
use crate::drive_remote_mounts::drive_s3_prefix;

struct SharedStore(Arc<InMemory>);

impl DriveObjectStores for SharedStore {
    fn open(&self, _bucket: &str) -> Result<Arc<dyn ObjectStore>, DrivePurgeError> {
        Ok(self.0.clone())
    }
}

struct UnreachableStores;

impl DriveObjectStores for UnreachableStores {
    fn open(&self, bucket: &str) -> Result<Arc<dyn ObjectStore>, DrivePurgeError> {
        Err(DrivePurgeError::Client {
            bucket: bucket.to_owned(),
            source: object_store::Error::Generic {
                store: "test",
                source: "unreachable".into(),
            },
        })
    }
}

fn test_drive_record(tenant: &str, name: &str, bucket: &str) -> Result<DriveRecord, Box<dyn std::error::Error>> {
    Ok(DriveRecord {
        id: DriveId {
            tenant: TenantId::new(tenant)?,
            name: trogon_sandbox_contract::DriveName::new(name)?,
        },
        tenant: TenantId::new(tenant)?,
        size_gib: 10,
        created_at: std::time::SystemTime::now(),
        attached_lease: None,
        storage: DriveStorage::S3 {
            bucket: bucket.to_string(),
        },
    })
}

async fn put(store: &InMemory, prefix: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    store
        .put(
            &ObjectPath::from(format!("{prefix}{name}")),
            bytes::Bytes::from_static(b"data").into(),
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn purge_removes_only_objects_under_target_drive_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemory::new());
    let record = test_drive_record("acme", "data0", "test-bucket")?;
    let other = test_drive_record("acme", "data1", "test-bucket")?;
    let other_prefix = drive_s3_prefix(&other.id);
    put(&store, &drive_s3_prefix(&record.id), "file.bin").await?;
    put(&store, &other_prefix, "file.bin").await?;

    purge_drive(&SharedStore(store.clone()), &record).await?;

    let mut remaining = store.list(None);
    let mut paths = Vec::new();
    while let Some(meta) = remaining.next().await {
        paths.push(meta?.location.to_string());
    }
    assert_eq!(paths.len(), 1);
    assert!(paths[0].starts_with(other_prefix.trim_end_matches('/')));
    Ok(())
}

#[tokio::test]
async fn provider_owned_volumes_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut record = test_drive_record("acme", "data0", "test-bucket")?;
    record.storage = DriveStorage::ProviderVolume {
        provider_volume: trogon_sandbox_contract::ProviderVolumeRef {
            provider: trogon_sandbox_contract::ProviderKind::Mock,
            volume_id: trogon_sandbox_contract::ProviderVolumeId::new("vol-1")?,
        },
    };
    let error = S3DriveDataPurger
        .purge(&record)
        .await
        .err()
        .ok_or("expected a refusal")?;
    assert!(matches!(error, DrivePurgeError::ProviderOwned));
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the test asserts the operator-facing message text itself"
    )
)]
async fn a_bucket_that_cannot_be_opened_fails_the_purge() -> Result<(), Box<dyn std::error::Error>> {
    let record = test_drive_record("acme", "data0", "test-bucket")?;
    let error = purge_drive(&UnreachableStores, &record)
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(error.to_string().contains("test-bucket"));
    Ok(())
}

#[tokio::test]
async fn noop_purger_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    NoopDriveDataPurger
        .purge(&test_drive_record("acme", "data0", "test-bucket")?)
        .await?;
    Ok(())
}

#[test]
fn env_stores_open_a_bucket_without_touching_the_network() -> Result<(), Box<dyn std::error::Error>> {
    EnvS3ObjectStores.open("test-bucket")?;
    Ok(())
}
