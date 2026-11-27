/// An API key store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum ApiKeyStoreError {
    #[error("API key store backend error: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("API key record is malformed: {0}")]
    Record(#[from] serde_json::Error),
    #[error("could not read secure random bytes: {0}")]
    Entropy(#[source] rand_core::Error),
}

impl ApiKeyStoreError {
    pub(crate) fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
