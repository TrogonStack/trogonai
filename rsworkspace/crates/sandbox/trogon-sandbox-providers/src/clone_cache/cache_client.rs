use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{
    CloneCacheConfig, KmsKeyId, VolumeClient, VolumeEndpoint, VolumeRegion, volume_client_for_clone_cache,
};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::VolumeCacheError;
use crate::volume_cache_env::VolumeCacheEnv;
use crate::volume_cache_env::constants::{BUCKET_ENV, ENDPOINT_ENV, KMS_KEY_ID_ENV, REGION_ENV};

pub(super) fn client_from_env() -> Result<VolumeClient, VolumeCacheError> {
    client_from(&SystemEnv)
}

pub(super) fn client_from(env: &impl ReadEnv) -> Result<VolumeClient, VolumeCacheError> {
    let settings = VolumeCacheEnv::from_env(env).ok_or(VolumeCacheError::BucketUnset)?;
    let config = CloneCacheConfig {
        bucket: S3BucketName::new(settings.bucket).map_err(|error| setting(BUCKET_ENV, error))?,
        region: VolumeRegion::new(settings.region).map_err(|error| setting(REGION_ENV, error))?,
        endpoint: settings
            .endpoint
            .map(VolumeEndpoint::new)
            .transpose()
            .map_err(|error| setting(ENDPOINT_ENV, error))?,
        kms_key_id: settings
            .kms_key_id
            .map(KmsKeyId::new)
            .transpose()
            .map_err(|error| setting(KMS_KEY_ID_ENV, error))?,
    };
    volume_client_for_clone_cache(&config).map_err(|error| VolumeCacheError::Client(Box::new(error)))
}

fn setting(variable: &'static str, error: impl std::error::Error + Send + Sync + 'static) -> VolumeCacheError {
    VolumeCacheError::InvalidSetting {
        variable,
        source: Box::new(error),
    }
}
