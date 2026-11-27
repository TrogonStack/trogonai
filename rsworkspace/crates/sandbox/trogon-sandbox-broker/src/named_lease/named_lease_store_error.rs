use std::error::Error;

/// Why a named-lease store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum NamedLeaseStoreError {
    /// The NATS KV bucket rejected or failed the operation.
    #[error("{0}")]
    Nats(Box<dyn Error + Send + Sync>),
    /// A stored claim could not be encoded or decoded.
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
    /// Concurrent writers kept winning the revision check.
    #[error("named lease {operation} exhausted CAS retries")]
    CasRetriesExhausted { operation: &'static str },
    /// Claiming kept racing with releases of the same name.
    #[error("named lease claim exhausted retries")]
    ClaimRetriesExhausted,
    /// The store has no exact-owner release.
    #[error("exact-owner named lease release is unavailable")]
    ExactOwnerReleaseUnavailable,
}

impl NamedLeaseStoreError {
    pub(crate) fn nats(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Nats(Box::new(error))
    }
}
