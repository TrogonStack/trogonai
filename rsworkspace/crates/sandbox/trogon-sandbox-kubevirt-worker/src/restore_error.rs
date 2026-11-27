use trogon_sandbox_cosign::SnapshotPullError;
use trogon_sandbox_oci::OciError;

/// A snapshot could not be staged for a DataVolume import.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("the snapshot signature was not accepted: {0}")]
    Verify(Box<SnapshotPullError>),
    #[error("pulling the snapshot failed: {0}")]
    Pull(#[from] OciError),
    #[error("the snapshot manifest has no layers")]
    NoLayers,
    #[error("restore staging needs an S3 volume backend")]
    UnsupportedBackend,
    #[error("the staging store failed: {0}")]
    Store(#[from] object_store::Error),
}
