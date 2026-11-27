/// A Daytona API endpoint was refused.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DaytonaEndpointError {
    #[error("the endpoint is not a URL: {0}")]
    Unparsable(#[from] url::ParseError),
    #[error("remote Daytona API endpoints require HTTPS")]
    InsecureScheme,
    #[error("the endpoint must not carry credentials, a query, or a fragment")]
    NotPlain,
}
