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
    #[error("peer event stream must not replace retained history through rollups")]
    Rollup,
    #[error("peer event stream must not admit per-message expiry")]
    MessageTtl,
    #[error("peer event stream must not import another stream's history")]
    Sources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerSnapshotConfigMismatch {
    #[error("peer snapshot bucket must retain one revision per key")]
    History,
    #[error("peer snapshot byte budget differs from its declared bound")]
    MaxBytes,
    #[error("peer snapshot value limit differs from its declared bound")]
    MaxValueSize,
    #[error("peer snapshot bucket must not expire snapshots")]
    MaxAge,
    #[error("peer snapshot bucket must use file storage")]
    Storage,
    #[error("peer snapshot subjects differ from their tenant bucket")]
    Subjects,
    #[error("peer snapshot bucket must reject new writes when full")]
    Discard,
    #[error("peer snapshot bucket must not import another bucket's history")]
    Sources,
}

#[derive(Debug, thiserror::Error)]
pub enum PeerStoreError {
    #[error("peer event stream provisioning failed: {0}")]
    Stream(#[from] EnsureStreamError),
    #[error("peer snapshot bucket provisioning failed: {0}")]
    Bucket(#[from] EnsureBucketError),
    #[error("peer event stream has unsafe existing configuration: {0}")]
    UnsafeConfig(#[from] PeerStreamConfigMismatch),
    #[error("peer snapshot bucket has unsafe existing configuration: {0}")]
    UnsafeSnapshotConfig(#[from] PeerSnapshotConfigMismatch),
    #[error("peer event subject configuration failed: {0}")]
    Subject(#[from] super::PeerSubjectError),
}
