/// A bearer token does not satisfy the rules [`BearerToken`](crate::BearerToken) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum BearerTokenError {
    #[error("bearer token must not be empty")]
    Empty,
    #[error("bearer token must contain only visible ASCII characters")]
    InvalidCharacter,
}
