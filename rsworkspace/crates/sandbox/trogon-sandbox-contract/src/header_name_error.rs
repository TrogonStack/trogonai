/// A string is not an HTTP header name [`HeaderName`](crate::HeaderName) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum HeaderNameError {
    #[error("HTTP header name must not be empty")]
    Empty,
    #[error("HTTP header name must be at most {max} bytes")]
    TooLong { max: usize },
    #[error("HTTP header name must be a token of letters, digits, and the characters !#$%&'*+-.^_`|~")]
    NotAToken,
}
