/// A string is not a valid [`SshSessionId`](crate::SshSessionId).
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SshSessionIdError {
    #[error("ssh session id must hold 1 to {max} bytes")]
    InvalidLength { max: usize },
    #[error("ssh session id must contain only ASCII letters, digits, '-' or '_'")]
    InvalidCharacters,
}
