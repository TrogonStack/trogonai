use std::path::PathBuf;

use trogon_sandbox_contract::S3BucketName;

use crate::{StoreResourceName, VolumeEndpoint, VolumeRegion};

/// The object store a volume lives in.
#[derive(Debug, Clone)]
pub enum VolumeBackend {
    S3 {
        bucket: S3BucketName,
        region: VolumeRegion,
        endpoint: Option<VolumeEndpoint>,
    },
    Gcs {
        bucket: StoreResourceName,
    },
    AzureBlob {
        container: StoreResourceName,
        account: StoreResourceName,
    },
    LocalFs {
        root: PathBuf,
    },
}
