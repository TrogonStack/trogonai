use thiserror::Error;

/// Why the launch-key journal or its cancellation custody could not answer.
#[derive(Debug, Error)]
pub enum CommandLaunchError {
    /// The store keeps no retained cancellation authority.
    #[error("retained command cancellation authority is unavailable")]
    CustodyUnsupported,
    /// The launch the authority belongs to is not in the journal.
    #[error("command launch is unavailable")]
    LaunchUnavailable,
    /// The authority was recorded for a different launch than the pending one.
    #[error("retained authority does not match its pending launch")]
    AuthorityMismatch,
    /// The authority failed its structural checks.
    #[error("invalid retained command cancellation authority")]
    InvalidAuthority,
    /// The stored authority is not a put or is over its byte limit.
    #[error("retained command authority is unreadable or exceeds its byte limit")]
    AuthorityUnreadable,
    /// An authority existed a moment ago and is gone.
    #[error("retained command cancellation authority disappeared")]
    AuthorityDisappeared,
    /// The custody bucket could neither be created nor loaded.
    #[error("command custody unavailable: create={create}; load={load}")]
    CustodyUnavailable { create: String, load: String },
    /// The custody bucket was not initialised.
    #[error("command custody is unavailable")]
    CustodyMissing,
    /// The custody bucket is not bounded, nonexpiring file storage without eviction.
    #[error("command custody requires bounded nonexpiring file authority without eviction")]
    CustodyUnbounded,
    /// Claiming kept racing with a removal of the same key.
    #[error("launch key claim kept racing with its removal")]
    ClaimRace,
    /// A journal entry did not encode or decode.
    #[error("command launch entry did not encode or decode: {0}")]
    Codec(#[from] serde_json::Error),
    /// The key-value backend failed.
    #[error("command launch backend failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl CommandLaunchError {
    /// Wraps a backend failure.
    pub fn backend(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}
