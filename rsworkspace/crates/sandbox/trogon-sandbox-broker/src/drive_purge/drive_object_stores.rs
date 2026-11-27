use std::sync::Arc;

use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;

use super::DrivePurgeError;

/// Opens the object store a drive's data lives in.
pub trait DriveObjectStores: Send + Sync {
    fn open(&self, bucket: &str) -> Result<Arc<dyn ObjectStore>, DrivePurgeError>;
}

/// Opens S3 buckets with the credentials and region the process environment names.
pub struct EnvS3ObjectStores;

impl DriveObjectStores for EnvS3ObjectStores {
    fn open(&self, bucket: &str) -> Result<Arc<dyn ObjectStore>, DrivePurgeError> {
        let store = AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .build()
            .map_err(|source| DrivePurgeError::Client {
                bucket: bucket.to_owned(),
                source,
            })?;
        Ok(Arc::new(store))
    }
}
