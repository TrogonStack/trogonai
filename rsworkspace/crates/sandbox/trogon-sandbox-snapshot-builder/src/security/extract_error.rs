use super::ScanLimitsError;

/// Why a Firecracker rootfs could not be turned into a scannable tree.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("extraction i/o failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("extraction job is not valid JSON: {0}")]
    Job(#[from] serde_json::Error),
    #[error("ext4 filesystem: {0}")]
    Ext4(#[from] ext4_view::Ext4Error),
    #[error("filesystem name is not valid UTF-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Limits(#[from] ScanLimitsError),
    #[error("extraction deadline exceeded")]
    DeadlineExceeded,
    #[error("starting isolated rootfs extractor: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("isolated rootfs extraction failed: {status}: {stderr}")]
    ProcessFailed { status: String, stderr: String },
    #[error("{0}")]
    Rejected(&'static str),
}
