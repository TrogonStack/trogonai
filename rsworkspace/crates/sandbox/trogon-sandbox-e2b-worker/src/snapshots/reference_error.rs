use std::time::SystemTimeError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum SnapshotReferenceError {
    #[error("snapshot does not identify an E2B reference")]
    NotE2b,
    #[error("native snapshot has expired")]
    Expired,
    #[error("system clock is before the Unix epoch")]
    Clock(#[from] SystemTimeError),
}
