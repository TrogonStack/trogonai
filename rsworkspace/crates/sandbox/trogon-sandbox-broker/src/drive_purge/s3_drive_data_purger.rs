use async_trait::async_trait;
use futures::StreamExt;
use object_store::ObjectStoreExt;
use object_store::path::Path as ObjectPath;
use trogon_sandbox_contract::DriveRecord;

use super::{DriveDataPurger, DriveObjectStores, DrivePurgeError, EnvS3ObjectStores};
use crate::drive_remote_mounts::drive_s3_prefix;

/// Purger that deletes every object under the drive's prefix in its S3 bucket.
pub struct S3DriveDataPurger;

#[async_trait]
impl DriveDataPurger for S3DriveDataPurger {
    async fn purge(&self, record: &DriveRecord) -> Result<(), DrivePurgeError> {
        purge_drive(&EnvS3ObjectStores, record).await
    }
}

/// Deletes every object under the drive's prefix in the bucket `stores` opens for it.
pub(super) async fn purge_drive(stores: &dyn DriveObjectStores, record: &DriveRecord) -> Result<(), DrivePurgeError> {
    let bucket = record.storage.s3_bucket().ok_or(DrivePurgeError::ProviderOwned)?;
    let store = stores.open(bucket)?;
    let prefix = ObjectPath::from(drive_s3_prefix(&record.id));
    let mut stream = store.list(Some(&prefix));
    while let Some(meta) = stream.next().await {
        let meta = meta.map_err(|source| DrivePurgeError::List {
            prefix: prefix.to_string(),
            source,
        })?;
        store
            .delete(&meta.location)
            .await
            .map_err(|source| DrivePurgeError::Delete {
                location: meta.location.to_string(),
                source,
            })?;
    }
    Ok(())
}
