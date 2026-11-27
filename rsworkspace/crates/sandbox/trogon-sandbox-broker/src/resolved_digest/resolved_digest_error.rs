/// A resolved-digest record could not be stored, read or purged.
#[derive(Debug, thiserror::Error)]
pub enum ResolvedDigestError {
    #[error("resolved digest store failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("resolved digest record is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
}

impl ResolvedDigestError {
    pub(crate) fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
