use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCacheError {
    Conflict,
    Backend(String),
    InvalidKey(String),
}

impl fmt::Display for SchemaCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => f.write_str("schema cache entry conflict"),
            Self::Backend(message) => write!(f, "schema cache backend error: {message}"),
            Self::InvalidKey(message) => write!(f, "invalid schema cache key: {message}"),
        }
    }
}

impl std::error::Error for SchemaCacheError {}
