/// A string is not a request path prefix [`PathPrefix`](crate::PathPrefix) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum PathPrefixError {
    #[error("a path prefix starts with '/'")]
    NotAbsolute,
    #[error("a path prefix is at most {max} bytes")]
    TooLong { max: usize },
    #[error("'{0}' is not allowed in a path prefix")]
    Forbidden(char),
    #[error("a path prefix has no dot segments or empty segments")]
    Ambiguous,
}
