/// A string is not an HTTP method [`HttpMethod`](crate::HttpMethod) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum HttpMethodError {
    #[error("HTTP method must not be empty")]
    Empty,
    #[error("HTTP method must be at most {max} bytes")]
    TooLong { max: usize },
    #[error("HTTP method must be a token of letters, digits, and the characters !#$%&'*+-.^_`|~")]
    NotAToken,
}
