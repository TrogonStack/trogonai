use async_nats::jetstream;
use async_nats::jetstream::kv;

/// Why a live snapshot projection read or write failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotLiveError {
    #[error("conflicting snapshot projection revision")]
    ConflictingRevision,
    #[error("versioned snapshot projection removal is unavailable")]
    RemovalUnavailable,
    #[error("tenant is required for a shared snapshot digest")]
    AmbiguousDigest,
    #[error("snapshot projection revision overflow")]
    RevisionOverflow,
    #[error("snapshot projection owner mismatch")]
    OwnerMismatch,
    #[error("snapshot tenant exceeds its storage bound")]
    TenantTooLong,
    #[error("snapshot live store must not evict projections under capacity pressure")]
    EvictionPolicy,
    #[error("snapshot projection CAS retries exhausted")]
    RetriesExhausted,
    #[error("snapshot projection bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("snapshot projection read failed: {0}")]
    Read(#[from] kv::EntryError),
    #[error("snapshot projection key listing failed: {0}")]
    Keys(#[from] kv::HistoryError),
    #[error("snapshot projection key stream failed: {0}")]
    KeyStream(#[from] kv::WatcherError),
    #[error("snapshot projection update failed: {0}")]
    Update(#[from] kv::UpdateError),
    #[error("snapshot projection create failed: {0}")]
    Create(#[from] kv::CreateError),
    #[error("snapshot projection is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
}
