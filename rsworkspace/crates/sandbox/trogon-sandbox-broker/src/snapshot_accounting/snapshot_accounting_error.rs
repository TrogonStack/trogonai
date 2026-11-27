use async_nats::jetstream;

/// Why opening the snapshot accounting store failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotAccountingError {
    #[error("snapshot authority bucket unavailable: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("snapshot authority store must refuse capacity pressure without expiring or evicting receipts")]
    BucketPolicy,
    #[error("snapshot publication queue unavailable: {0}")]
    Stream(#[from] jetstream::context::CreateStreamError),
    #[error("snapshot publication queue has incompatible durability or bounds")]
    StreamPolicy,
}
