use async_nats::jetstream::context::CreateKeyValueError;
use async_nats::jetstream::kv::{CreateError, EntryError, HistoryError, UpdateError, WatcherError};

/// Why the snapshot metadata bucket could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotKvError {
    #[error("snapshot metadata bucket unavailable: {0}")]
    Bucket(#[from] CreateKeyValueError),
    #[error("snapshot metadata read failed: {0}")]
    Entry(#[from] EntryError),
    #[error("snapshot metadata create failed: {0}")]
    Create(#[from] CreateError),
    #[error("snapshot metadata update failed: {0}")]
    Update(#[from] UpdateError),
    #[error("snapshot metadata listing failed: {0}")]
    Listing(#[from] HistoryError),
    #[error("snapshot metadata listing failed: {0}")]
    Key(#[from] WatcherError),
    #[error("snapshot metadata is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("snapshot metadata identity mismatch")]
    IdentityMismatch,
    #[error("conflicting snapshot metadata at one capture time")]
    ConflictingMetadata,
    #[error("snapshot metadata CAS retries exhausted")]
    RetriesExhausted,
    #[error("tenant required for shared snapshot metadata")]
    TenantRequired,
}
