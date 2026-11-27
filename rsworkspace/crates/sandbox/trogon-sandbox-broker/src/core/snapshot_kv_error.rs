use async_nats::jetstream::context::CreateKeyValueError;
use async_nats::jetstream::kv::{CreateError, EntryError, UpdateError};

/// Why a [`SnapshotKvReader`](super::SnapshotKvReader) call failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotKvError {
    #[error("snapshot metadata writes are unavailable")]
    WritesUnavailable,
    #[error("snapshot metadata bucket unavailable: {0}")]
    Bucket(#[source] CreateKeyValueError),
    #[error("snapshot metadata read failed: {0}")]
    Read(#[source] EntryError),
    #[error("snapshot metadata update failed: {0}")]
    Update(#[source] UpdateError),
    #[error("snapshot metadata create failed: {0}")]
    Create(#[source] CreateError),
    #[error("snapshot metadata decode failed: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("snapshot metadata encode failed: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("snapshot metadata identity mismatch")]
    IdentityMismatch,
    #[error("conflicting snapshot metadata at one capture time")]
    ConflictingCapture,
    #[error("snapshot metadata CAS retries exhausted")]
    CasRetriesExhausted,
    #[error("tenant required for shared snapshot metadata")]
    TenantRequired,
}
