/// A token does not satisfy the rules [`SpiceDbToken`](crate::SpiceDbToken) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SpiceDbTokenError {
    #[error("permission service token must not be empty")]
    Empty,
    #[error("permission service token cannot be sent as a request header")]
    NotHeaderSafe,
}
