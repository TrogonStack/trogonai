/// A drive name does not satisfy the wire grammar.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum DriveNameError {
    #[error("drive name must be 1-64 characters")]
    InvalidLength,
    #[error("drive name must not contain '/', '\\\\', or '..'")]
    InvalidCharacters,
}
