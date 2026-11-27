use crate::{KubeVirtOpsError, WarmDiskSourceError};

/// Why the warm pool could not start a warm VM.
#[derive(Debug, thiserror::Error)]
pub enum WarmPoolManagerError {
    #[error(transparent)]
    Ops(#[from] KubeVirtOpsError),
    #[error(transparent)]
    DiskSource(#[from] WarmDiskSourceError),
    #[error("warm pool prewarm requires SANDBOX_VOLUME_S3_BUCKET configuration")]
    VolumeUnconfigured,
    #[error("warm VM has no returned revision")]
    VmRevisionMissing,
}
