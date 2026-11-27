use crate::BackendError;

/// An audit event could not be recorded.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("audit event could not be encoded")]
    Codec(#[from] serde_json::Error),
    #[error(transparent)]
    Backend(#[from] BackendError),
}
