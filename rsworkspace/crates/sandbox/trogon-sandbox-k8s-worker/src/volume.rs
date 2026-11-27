use trogon_sandbox_contract::{S3BucketName, VolumeMode};
use trogon_sandbox_volumes::{
    KmsKeyId, VolumeBackend, VolumeClient, VolumeClientConfig, VolumeEndpoint, VolumePrefix, VolumeRegion,
};
use trogon_std::env::ReadEnv;

use crate::VolumeSyncError;
use crate::constants::{DEFAULT_S3_REGION, S3_ENDPOINT_ENV, S3_KMS_KEY_ID_ENV, S3_REGION_ENV};
use crate::snapshot::tar_directory;

pub fn volume_client_config_for_bucket(
    env: &impl ReadEnv,
    bucket: &str,
) -> Result<VolumeClientConfig, VolumeSyncError> {
    let region = env.var(S3_REGION_ENV).unwrap_or_else(|_| DEFAULT_S3_REGION.to_string());
    let endpoint = env.var(S3_ENDPOINT_ENV).ok().map(VolumeEndpoint::new).transpose()?;
    let kms_key_id = env.var(S3_KMS_KEY_ID_ENV).ok().map(KmsKeyId::new).transpose()?;
    Ok(VolumeClientConfig {
        backend: VolumeBackend::S3 {
            bucket: S3BucketName::new(bucket)?,
            region: VolumeRegion::new(region)?,
            endpoint,
        },
        kms_key_id,
    })
}

pub async fn sync_s3_prefix_to_tar(
    env: &impl ReadEnv,
    bucket: &str,
    prefix: &str,
    mode: VolumeMode,
) -> Result<Vec<u8>, VolumeSyncError> {
    let config = volume_client_config_for_bucket(env, bucket)?;
    let client = VolumeClient::with_mode(config, mode).map_err(VolumeSyncError::Client)?;
    let scratch = tempfile::tempdir().map_err(VolumeSyncError::TempDir)?;
    client
        .pull_to_dir(&VolumePrefix::new(prefix)?, scratch.path())
        .await
        .map_err(VolumeSyncError::Pull)?;
    Ok(tar_directory(scratch.path())?)
}
