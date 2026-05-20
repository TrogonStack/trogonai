use super::{header_name::HeaderNameError, header_value::HeaderValueError};

/// Error returned while converting raw header entries into [`Headers`](crate::Headers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromEntriesError {
    /// A raw header name failed validation.
    InvalidName { name: String, source: HeaderNameError },
    /// A raw header value failed validation.
    InvalidValue { name: String, source: HeaderValueError },
}

impl std::fmt::Display for FromEntriesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName { name, .. } => write!(f, "event header name '{name}' is not valid"),
            Self::InvalidValue { name, .. } => {
                write!(f, "event header value for '{name}' is not valid")
            }
        }
    }
}

impl std::error::Error for FromEntriesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidName { source, .. } => Some(source),
            Self::InvalidValue { source, .. } => Some(source),
        }
    }
}
