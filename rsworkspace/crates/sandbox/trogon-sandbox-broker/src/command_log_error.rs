/// Why a log read failed. `InvalidExecution` is the caller's fault; `Backend`
/// is the store's.
#[derive(Debug, thiserror::Error)]
pub enum CommandLogError {
    #[error("execution id must be 1 to 128 characters of [A-Za-z0-9_-]")]
    InvalidExecution,
    #[error("command log store failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl CommandLogError {
    /// Wraps a backend failure.
    pub fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
