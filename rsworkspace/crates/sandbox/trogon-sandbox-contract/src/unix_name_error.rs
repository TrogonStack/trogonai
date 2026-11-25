/// A Unix user or group name does not match `^[a-z_][a-z0-9_-]*$` within 32 bytes.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum UnixNameError {
    #[error("name must be 1-32 characters")]
    InvalidLength,
    #[error("name must start with [a-z_] and contain only [a-z0-9_-]")]
    InvalidCharacters,
}
