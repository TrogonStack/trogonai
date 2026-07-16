use super::{header_name::HeaderNameError, header_value::HeaderValueError};

/// Error returned while converting raw header entries into [`Headers`](crate::Headers).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FromEntriesError {
    /// A raw header name failed validation.
    #[error("event header name '{name}' is not valid")]
    InvalidName {
        /// The raw name that failed validation.
        name: String,
        /// The underlying validation failure.
        #[source]
        source: HeaderNameError,
    },
    /// A raw header value failed validation.
    #[error("event header value for '{name}' is not valid")]
    InvalidValue {
        /// The name of the header whose value failed validation.
        name: String,
        /// The underlying validation failure.
        #[source]
        source: HeaderValueError,
    },
}
