/// An authorization check did not allow the request.
#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    #[error("permission denied")]
    Denied,
    #[error("authorization backend error: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl AuthzError {
    pub(crate) fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
