use crate::{BackendError, StaleCommit};

/// A committed-policy store could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum PolicyStoreError {
    #[error("committed policy could not be encoded or decoded")]
    Codec(#[from] serde_json::Error),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("the committed policy for the lease kept changing; giving up")]
    Contended,
    #[error(transparent)]
    Stale(#[from] StaleCommit),
}
