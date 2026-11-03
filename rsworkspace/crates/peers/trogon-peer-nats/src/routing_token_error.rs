#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoutingTokenError {
    #[error("routing token must not be empty")]
    Empty,
    #[error("routing token exceeds 128 bytes")]
    TooLong,
    #[error("routing token contains an unsupported character")]
    InvalidCharacter,
}
