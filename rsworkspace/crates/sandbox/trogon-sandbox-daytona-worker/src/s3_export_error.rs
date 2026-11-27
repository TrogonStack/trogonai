use trogon_sandbox_volumes::{KmsKeyIdError, VolumeError};

use crate::WorkspaceArchiveError;

/// Why exporting a workspace to S3 failed.
#[derive(Debug, thiserror::Error)]
pub enum S3ExportError {
    #[error("cannot archive the workspace in the sandbox: {0}")]
    Archive(#[from] WorkspaceArchiveError),
    #[error("cannot prepare the local scratch directory: {0}")]
    Scratch(#[source] std::io::Error),
    #[error("cannot unpack the workspace archive: {0}")]
    Unpack(#[source] std::io::Error),
    #[error("the unpack task did not finish: {0}")]
    UnpackTask(#[source] tokio::task::JoinError),
    #[error("invalid KMS key id: {0}")]
    KmsKey(#[from] KmsKeyIdError),
    #[error("cannot push the workspace to the volume: {0}")]
    Volume(#[from] VolumeError),
}
