use async_nats::jetstream;
use async_nats::jetstream::kv;

/// Why a snapshot catalog read or prune failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotCatalogError {
    #[error("snapshot catalog bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("snapshot catalog key listing failed: {0}")]
    Keys(#[from] kv::HistoryError),
    #[error("snapshot catalog key stream failed: {0}")]
    KeyStream(#[from] kv::WatcherError),
    #[error("snapshot catalog entry read failed: {0}")]
    Read(#[from] kv::EntryError),
    #[error("snapshot catalog entry delete failed: {0}")]
    Delete(#[from] kv::UpdateError),
    #[error("snapshot catalog record is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
}
