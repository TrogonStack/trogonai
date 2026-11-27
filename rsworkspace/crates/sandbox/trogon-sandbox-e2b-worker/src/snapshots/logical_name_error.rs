#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotLogicalNameError {
    #[error("snapshot logical name is invalid")]
    Invalid,
}
