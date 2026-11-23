/// A snapshot tag name does not satisfy the wire grammar.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotTagError {
    #[error("tag name must be 1-64 characters")]
    InvalidLength,
    #[error("tag name must match [a-zA-Z0-9._-]")]
    InvalidCharacters,
}
