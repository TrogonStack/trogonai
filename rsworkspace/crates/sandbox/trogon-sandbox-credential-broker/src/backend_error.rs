use std::error::Error;

/// A storage or messaging backend failed. It wraps the backend's own error so callers can log it without the
/// broker depending on the backend's error types.
#[derive(Debug, thiserror::Error)]
#[error("backend failed: {source}")]
pub struct BackendError {
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl BackendError {
    pub fn new(source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            source: Box::new(source),
        }
    }
}
