/// Why publishing an entity lifecycle event, or preparing the stream, failed.
#[derive(Debug, thiserror::Error)]
pub enum EntityEventSinkError {
    #[error("entity event could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("entity event stream failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl EntityEventSinkError {
    /// Wraps a backend failure.
    pub fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
