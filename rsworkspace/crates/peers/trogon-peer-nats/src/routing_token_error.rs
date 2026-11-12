use trogon_nats::SubjectTokenViolationError;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RoutingTokenError {
    #[error("routing token must not be empty")]
    Empty,
    #[error("routing token exceeds 128 bytes")]
    TooLong,
    #[error("routing token contains an unsupported character")]
    InvalidCharacter,
    #[error("routing token is not a valid NATS token: {0}")]
    Nats(#[source] SubjectTokenViolationError),
}
