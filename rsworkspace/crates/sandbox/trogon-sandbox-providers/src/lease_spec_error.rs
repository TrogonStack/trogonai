use crate::CommitShaError;

/// Why a lease request cannot become a provider launch spec.
#[derive(Debug, thiserror::Error)]
pub enum LeaseSpecError {
    #[error("invalid resolved_sha: {0}")]
    InvalidResolvedSha(#[source] CommitShaError),
    #[error("invalid base_sha: {0}")]
    InvalidBaseSha(#[source] CommitShaError),
    #[error("host_mount path must not be empty")]
    HostMountPathEmpty,
    #[error("host_mount path must be absolute: {path}")]
    HostMountPathNotAbsolute { path: String },
    #[error("s3_volume workspaces are not supported by the providers yet")]
    S3VolumeUnsupported,
}
