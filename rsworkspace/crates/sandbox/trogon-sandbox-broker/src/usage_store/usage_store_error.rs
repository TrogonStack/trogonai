use async_nats::jetstream;
use async_nats::jetstream::kv;

/// Why a lease usage store read or write failed.
#[derive(Debug, thiserror::Error)]
pub enum UsageStoreError {
    #[error("lease usage bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("lease usage entry read failed: {0}")]
    Read(#[from] kv::EntryError),
    #[error("lease usage entry update failed: {0}")]
    Update(#[from] kv::UpdateError),
    #[error("lease usage entry create failed: {0}")]
    Create(#[from] kv::CreateError),
    #[error("lease usage scan consumer failed: {0}")]
    Consumer(#[from] jetstream::stream::ConsumerError),
    #[error("lease usage scan stream failed: {0}")]
    Scan(#[from] jetstream::consumer::StreamError),
    #[error("lease usage scan message failed: {0}")]
    ScanMessage(#[from] jetstream::consumer::push::OrderedError),
    #[error("lease usage scan message metadata unreadable: {0}")]
    MessageInfo(String),
    #[error("lease usage entry is not valid JSON: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("lease usage update exhausted CAS retries")]
    RetriesExhausted,
}
