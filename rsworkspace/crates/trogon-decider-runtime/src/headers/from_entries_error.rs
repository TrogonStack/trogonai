use super::{header_name::HeaderNameError, header_value::HeaderValueError};

/// Error returned while converting raw header entries into [`Headers`](crate::Headers).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FromEntriesError {
    /// A raw header name failed validation.
    #[error("event header name '{name}' is not valid")]
    InvalidName {
        name: String,
        #[source]
        source: HeaderNameError,
    },
    /// A raw header value failed validation.
    #[error("event header value for '{name}' is not valid")]
    InvalidValue {
        name: String,
        #[source]
        source: HeaderValueError,
    },
}
