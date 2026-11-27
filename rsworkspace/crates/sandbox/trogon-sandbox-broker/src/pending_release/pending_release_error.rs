use std::error::Error;

/// Why a pending-release store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum PendingReleaseError {
    /// The NATS KV bucket rejected or failed the operation.
    #[error("{0}")]
    Nats(Box<dyn Error + Send + Sync>),
    /// A marker could not be encoded or decoded.
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
}

impl PendingReleaseError {
    pub(crate) fn nats(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Nats(Box::new(error))
    }
}
