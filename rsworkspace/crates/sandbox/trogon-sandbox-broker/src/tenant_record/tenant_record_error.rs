/// A per-tenant KV record could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum TenantRecordError {
    #[error("tenant record store failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("tenant record is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
}

impl TenantRecordError {
    pub(crate) fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
