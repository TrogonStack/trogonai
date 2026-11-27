use std::error::Error;

/// A failure of the key-value backend behind an operation store.
#[derive(Debug, thiserror::Error)]
pub enum OperationStoreError {
    #[error("{0}")]
    Backend(Box<dyn Error + Send + Sync>),
}

impl OperationStoreError {
    pub fn backend(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(source))
    }
}
