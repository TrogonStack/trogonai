#[derive(Debug, thiserror::Error)]
#[error("owner tuple emit failed: {0}")]
pub struct OwnerEmitError(#[source] Box<dyn std::error::Error + Send + Sync>);

impl OwnerEmitError {
    pub fn new(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(source))
    }
}
