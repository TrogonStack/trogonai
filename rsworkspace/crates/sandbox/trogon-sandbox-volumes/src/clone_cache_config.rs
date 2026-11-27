use trogon_sandbox_contract::S3BucketName;

use crate::{KmsKeyId, VolumeEndpoint, VolumeRegion};

#[derive(Debug, Clone)]
pub struct CloneCacheConfig {
    pub bucket: S3BucketName,
    pub region: VolumeRegion,
    pub endpoint: Option<VolumeEndpoint>,
    pub kms_key_id: Option<KmsKeyId>,
}
