use crate::FlyRegionError;

/// A Fly snapshot reference was malformed or cannot be restored by this worker.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum FlySnapshotRefError {
    #[error("not a Fly snapshot reference")]
    NotFlyReference,
    #[error("invalid snapshot id in a Fly snapshot reference")]
    InvalidSnapshotId,
    #[error("invalid region in a Fly snapshot reference: {0}")]
    Region(FlyRegionError),
    #[error("invalid image digest in a Fly snapshot reference")]
    InvalidImageHash,
    #[error("Fly snapshot lives in region {snapshot} but this worker places Machines in {worker}")]
    ForeignRegion { snapshot: String, worker: String },
    #[error("Fly snapshot was taken from a different image than this worker serves")]
    ForeignImage,
}
