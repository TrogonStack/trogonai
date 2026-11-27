/// Why an actor index operation failed.
#[derive(Debug, thiserror::Error)]
pub enum ActorIndexError {
    #[error("actor claim exchange has no identity")]
    ExchangeWithoutIdentity,
    #[error("actor claim exchange identities differ")]
    ExchangeIdentitiesDiffer,
    #[error("atomic actor claims are not implemented")]
    AtomicClaimsUnsupported,
    #[error("invalid actor ownership: {0}")]
    InvalidEntry(#[source] serde_json::Error),
    #[error("actor ownership key differs from its identity")]
    EntryKeyMismatch,
    #[error("actor ownership could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("actor index storage failed: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ActorIndexError {
    pub fn storage(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Storage(Box::new(error))
    }
}
