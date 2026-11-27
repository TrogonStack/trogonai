/// A string is not an HTTP header value [`HeaderValue`](crate::HeaderValue) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum HeaderValueError {
    #[error("HTTP header value must not be empty")]
    Empty,
    #[error("HTTP header value must be at most {max} bytes")]
    TooLong { max: usize },
    #[error("HTTP header value must not contain control characters")]
    ControlCharacter,
}
