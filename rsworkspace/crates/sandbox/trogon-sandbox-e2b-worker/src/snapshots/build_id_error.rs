/// Why a string is not a canonical E2B snapshot build id.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotBuildIdError {
    #[error("invalid E2B snapshot build UUID")]
    Malformed,
    #[error("invalid canonical E2B snapshot build UUID")]
    NotCanonical,
}
