/// An endpoint does not satisfy the rules [`SpiceDbEndpoint`](crate::SpiceDbEndpoint) enforces.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SpiceDbEndpointError {
    #[error("permission service endpoint must not be empty")]
    Empty,
    #[error("permission service endpoint is not a valid URI: {0}")]
    InvalidUri(String),
}
