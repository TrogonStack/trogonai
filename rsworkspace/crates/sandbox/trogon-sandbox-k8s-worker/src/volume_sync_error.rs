use trogon_sandbox_contract::S3BucketNameError;
use trogon_sandbox_volumes::{KmsKeyIdError, VolumeEndpointError, VolumeError, VolumePrefixError, VolumeRegionError};

use crate::SnapshotTransferError;

/// Failures pulling an S3 volume prefix into a tar.
#[derive(Debug, thiserror::Error)]
pub enum VolumeSyncError {
    #[error("environment variable {name} is not valid Unicode")]
    EnvNotUnicode { name: &'static str },
    #[error("invalid S3 bucket name")]
    Bucket(#[from] S3BucketNameError),
    #[error("invalid S3 region")]
    Region(#[from] VolumeRegionError),
    #[error("invalid S3 endpoint")]
    Endpoint(#[from] VolumeEndpointError),
    #[error("invalid S3 KMS key id")]
    KmsKey(#[from] KmsKeyIdError),
    #[error("invalid volume prefix")]
    Prefix(#[from] VolumePrefixError),
    #[error("build volume client")]
    Client(#[source] VolumeError),
    #[error("create temp dir for s3 volume pull")]
    TempDir(#[source] std::io::Error),
    #[error("pull s3 volume prefix")]
    Pull(#[source] VolumeError),
    #[error(transparent)]
    Tar(#[from] SnapshotTransferError),
}
