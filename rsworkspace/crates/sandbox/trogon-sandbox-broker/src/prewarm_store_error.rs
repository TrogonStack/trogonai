use thiserror::Error;

/// Why the prewarm policy store could not answer.
#[derive(Debug, Error)]
pub enum PrewarmStoreError {
    /// A stored policy did not encode or decode.
    #[error("prewarm policy did not encode or decode: {0}")]
    Codec(#[from] serde_json::Error),
    /// The key-value backend failed.
    #[error("prewarm policy backend failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl PrewarmStoreError {
    /// Wraps a backend failure.
    pub fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
