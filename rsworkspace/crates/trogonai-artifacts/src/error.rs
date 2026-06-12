use thiserror::Error;
use trogonai_session_contracts::{ArtifactId, SessionId};
use trogonai_session_kernel::SessionKernelError;

#[derive(Debug, Error)]
pub enum ArtifactStoreError {
    #[error("artifact object store put failed for session {session_id}: {detail}")]
    ObjectStorePut {
        session_id: SessionId,
        detail: String,
    },

    #[error("artifact object store get failed for artifact {artifact_id}: {detail}")]
    ObjectStoreGet {
        artifact_id: ArtifactId,
        detail: String,
    },

    #[error("artifact object store delete failed for {artifact_id}: {detail}")]
    ObjectStoreDelete {
        artifact_id: String,
        detail: String,
    },

    #[error("artifact checksum mismatch for {artifact_id}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        artifact_id: ArtifactId,
        expected: String,
        actual: String,
    },

    #[error("artifact {artifact_id} has no object store reference")]
    MissingStorageRef { artifact_id: ArtifactId },

    #[error("artifact {artifact_id} is inline and cannot be retrieved from object store")]
    InlineArtifact { artifact_id: ArtifactId },

    #[error("session kernel error while emitting artifact_created: {0}")]
    Kernel(#[from] SessionKernelError),

    #[error("failed to read artifact bytes: {0}")]
    Read(std::io::Error),
}
