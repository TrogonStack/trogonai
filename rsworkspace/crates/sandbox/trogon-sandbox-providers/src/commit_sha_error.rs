/// Why a string is not a commit SHA.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitShaError {
    #[error("commit SHA must be a hex string between 4 and 64 characters, got length {0}")]
    InvalidLength(usize),
    #[error("commit SHA must contain only hex characters")]
    NonHex,
}
