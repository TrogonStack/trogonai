use async_nats::jetstream;
use async_nats::jetstream::kv;

/// Why a snapshot last-used read or write failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotLastUsedError {
    #[error("snapshot last-used bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("snapshot last-used write failed: {0}")]
    Put(#[from] kv::PutError),
    #[error("snapshot last-used read failed: {0}")]
    Read(#[from] kv::EntryError),
    #[error("snapshot last-used record is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
}
