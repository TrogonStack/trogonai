use trogon_decider_nats::{EnsureBucketError, EnsureStreamError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerStreamConfigMismatch {
    #[error("peer event stream must reject new writes when full")]
    Discard,
    #[error("peer event stream must retain accepted events")]
    Retention,
    #[error("peer event stream byte budget differs from its declared bound")]
    MaxBytes,
    #[error("peer event stream must not expire accepted events")]
    MaxAge,
    #[error("peer event stream must not evict by event count")]
    MaxMessages,
    #[error("peer event stream must not evict by per-peer event count")]
    MaxMessagesPerSubject,
    #[error("peer event stream must support atomic event batches")]
    AtomicPublish,
    #[error("peer event stream subject filter differs from its tenant scope")]
    Subjects,
    #[error("peer event stream must use file storage")]
    Storage,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerStoreError {
    #[error("peer event store is not implemented")]
    Unimplemented,
    #[error("peer event stream provisioning failed: {0}")]
    Stream(#[from] EnsureStreamError),
    #[error("peer snapshot bucket provisioning failed: {0}")]
    Bucket(#[from] EnsureBucketError),
    #[error("peer event stream has unsafe existing configuration: {0}")]
    UnsafeConfig(#[from] PeerStreamConfigMismatch),
}
