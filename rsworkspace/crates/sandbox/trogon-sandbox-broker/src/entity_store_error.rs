use thiserror::Error;
use trogon_sandbox_protocol::IllegalTransitionError;

/// Why an entity store call failed.
#[derive(Debug, Error)]
pub enum EntityStoreError {
    #[error("sandbox entity not found")]
    NotFound,
    #[error("sandbox entity name already taken")]
    NameTaken,
    #[error("revision conflict: expected revision does not match current value")]
    RevisionConflict,
    #[error("invalid entity argument: {0}")]
    InvalidArgument(String),
    #[error(transparent)]
    IllegalTransition(#[from] IllegalTransitionError),
    #[error("sandbox entity store backend error: entity store CAS failed after max retries")]
    CasRetriesExhausted,
    #[error("sandbox entity store backend error: entity name claim failed after max retries")]
    NameClaimRetriesExhausted,
    #[error("sandbox entity store backend error: entity name release failed after max retries")]
    NameReleaseRetriesExhausted,
    #[error("sandbox entity store backend error: entity delete failed after max retries")]
    DeleteRetriesExhausted,
    #[error("sandbox entity store backend error: corrupt entity name claim")]
    CorruptNameClaim,
    #[error("sandbox entity store backend error: decode entity: {0}")]
    DecodeEntity(#[source] serde_json::Error),
    #[error("sandbox entity store backend error: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl EntityStoreError {
    /// Wraps a backend failure.
    pub fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
