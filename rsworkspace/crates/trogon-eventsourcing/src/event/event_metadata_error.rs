#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventMetadataError {
    EmptyName,
    ReservedName { name: String },
    InvalidName { name: String },
    InvalidValue { name: String },
}

impl std::fmt::Display for EventMetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "event metadata name cannot be empty"),
            Self::ReservedName { name } => write!(f, "event metadata name '{name}' is reserved"),
            Self::InvalidName { name } => write!(f, "event metadata name '{name}' is not a valid header name suffix"),
            Self::InvalidValue { name } => write!(f, "event metadata value for '{name}' is not a valid header value"),
        }
    }
}

impl std::error::Error for EventMetadataError {}
