#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerSnapshotConfigMismatchError {
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
