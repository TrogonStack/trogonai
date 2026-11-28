use trogon_sandbox_contract::{S3BucketName, VolumeMode};
use trogon_sandbox_volumes::{
    KmsKeyId, VolumeBackend, VolumeClient, VolumeClientConfig, VolumeEndpoint, VolumePrefix, VolumeRegion,
};
use trogon_std::env::ReadEnv;

use crate::VolumeSyncError;
use crate::constants::{DEFAULT_S3_REGION, S3_ENDPOINT_ENV, S3_KMS_KEY_ID_ENV, S3_REGION_ENV};
use crate::snapshot::tar_directory;

/// The value of `name`, or `None` when it is unset. A value that is not valid
/// Unicode is refused rather than treated as unset, so a malformed endpoint or
/// KMS key never silently reverts to the unencrypted or default target.
fn optional_var(env: &impl ReadEnv, name: &'static str) -> Result<Option<String>, VolumeSyncError> {
    match env.var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(VolumeSyncError::EnvNotUnicode { name }),
    }
}

pub fn volume_client_config_for_bucket(
    env: &impl ReadEnv,
    bucket: &str,
) -> Result<VolumeClientConfig, VolumeSyncError> {
    let region = optional_var(env, S3_REGION_ENV)?.unwrap_or_else(|| DEFAULT_S3_REGION.to_string());
    let endpoint = optional_var(env, S3_ENDPOINT_ENV)?
        .map(VolumeEndpoint::new)
        .transpose()?;
    let kms_key_id = optional_var(env, S3_KMS_KEY_ID_ENV)?.map(KmsKeyId::new).transpose()?;
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

#[cfg(test)]
mod tests;
