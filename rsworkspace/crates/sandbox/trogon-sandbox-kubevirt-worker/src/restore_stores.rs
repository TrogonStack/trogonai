use std::sync::Arc;

use object_store::ObjectStore;
use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{VolumeEndpoint, VolumeRegion};

use crate::RestoreError;

/// Opens the object store a restore stages qcow2 images in.
pub trait RestoreStores: Send + Sync {
    /// The store of `bucket`, reached at `endpoint` when one is given.
    fn open(
        &self,
        bucket: &S3BucketName,
        region: &VolumeRegion,
        endpoint: Option<&VolumeEndpoint>,
    ) -> Result<Arc<dyn ObjectStore>, RestoreError>;
}
