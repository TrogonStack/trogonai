/// Why a provider could not take a snapshot.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotError {
    /// The provider has no snapshot support.
    #[error("snapshot not supported by this provider")]
    Unsupported,
}
