/// Why a template index operation failed.
#[derive(Debug, thiserror::Error)]
pub enum TemplateIndexError {
    #[error("template record could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("template index storage failed: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl TemplateIndexError {
    pub fn storage(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Storage(Box::new(error))
    }
}
