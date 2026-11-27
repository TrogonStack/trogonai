use async_nats::jetstream;
use async_nats::jetstream::kv;

/// Why a snapshot capture status read or write failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotCaptureStatusError {
    #[error("snapshot capture status bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("snapshot capture status write failed: {0}")]
    Put(#[from] kv::PutError),
    #[error("snapshot capture status read failed: {0}")]
    Read(#[from] kv::EntryError),
    #[error("snapshot capture status is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
}
