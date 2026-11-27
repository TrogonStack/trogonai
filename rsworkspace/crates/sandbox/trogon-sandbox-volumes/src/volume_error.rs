use std::path::PathBuf;
use std::process::ExitStatus;

use trogon_sandbox_contract::DigestError;

use crate::{ManifestKeyError, VolumeMetricsError, VolumePrefixError};

#[derive(Debug, thiserror::Error)]
pub enum VolumeError {
    #[error("object store: {0}")]
    Backend(#[from] object_store::Error),
    #[error("cannot build the {backend} store: {source}")]
    StoreBuild {
        backend: &'static str,
        #[source]
        source: object_store::Error,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Digest(#[from] DigestError),
    #[error(transparent)]
    Key(#[from] ManifestKeyError),
    #[error(transparent)]
    Prefix(#[from] VolumePrefixError),
    #[error(transparent)]
    Metrics(#[from] VolumeMetricsError),
    #[error("quota exceeded for bucket {bucket}: limit {limit_bytes} bytes")]
    QuotaExceeded { bucket: String, limit_bytes: u64 },
    #[error("FUSE mounts are not supported on this platform")]
    FuseUnsupported,
    #[error("failed to spawn the FUSE mount command: {0}")]
    FuseSpawnFailed(#[source] std::io::Error),
    #[error("FUSE mount command exited with status {status}: {stderr}")]
    FuseMountRejected { status: ExitStatus, stderr: String },
    #[error("FUSE mount readiness timeout for {}", path.display())]
    FuseMountTimeout { path: PathBuf },
    #[error("failed to unmount FUSE at {}", path.display())]
    FuseUnmountFailed { path: PathBuf },
}
