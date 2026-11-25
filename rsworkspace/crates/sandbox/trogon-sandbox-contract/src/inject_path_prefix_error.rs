/// A string is not a path prefix [`InjectPathPrefix`](crate::InjectPathPrefix) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum InjectPathPrefixError {
    #[error("path_prefix must start with '/'")]
    NotAbsolute,
    #[error("path_prefix must be at most {max} bytes")]
    TooLong { max: usize },
    #[error(
        "path_prefix may contain only unreserved characters, sub-delimiters, ':', '@' and '/' (no '%', '?', '#', spaces or control characters)"
    )]
    Character,
    #[error("path_prefix must not contain empty, '.' or '..' segments")]
    Segment,
    #[error("path_prefix must not end with '/' unless it is '/'")]
    TrailingSlash,
}
