/// Why an E2B configuration was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum E2bConfigError {
    #[error("remote endpoints require HTTPS")]
    InsecureEndpoint,
    #[error("endpoint URL must not carry credentials, a query, or a fragment")]
    DecoratedEndpoint,
    #[error("invalid E2B domain")]
    InvalidDomain,
    #[error("the E2B API key is empty")]
    EmptyApiKey,
    #[error("the E2B API key is not a valid header value")]
    InvalidApiKey,
}
