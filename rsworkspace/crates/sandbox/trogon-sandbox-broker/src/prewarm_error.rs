use thiserror::Error;

use crate::prewarm_store_error::PrewarmStoreError;

/// Why a prewarm tick or one of its dispatches failed.
#[derive(Debug, Error)]
pub enum PrewarmError {
    /// The policy store failed.
    #[error("prewarm policy store failed: {0}")]
    Store(#[from] PrewarmStoreError),
    /// The live snapshot index failed.
    #[error("live snapshot lookup failed: {0}")]
    LiveSnapshots(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The warm lease request could not be built.
    #[error(transparent)]
    Request(#[from] crate::prewarm::PrewarmRequestError),
}
