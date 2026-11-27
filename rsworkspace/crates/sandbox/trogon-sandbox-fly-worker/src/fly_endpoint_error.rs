/// A Fly API endpoint was refused.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum FlyEndpointError {
    #[error("the endpoint is not a URL: {0}")]
    Unparsable(#[from] url::ParseError),
    #[error("remote Fly API endpoints require HTTPS")]
    InsecureScheme,
    #[error("the endpoint must not carry credentials, a query, or a fragment")]
    NotPlain,
}
