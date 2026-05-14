#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventHeadersError {
    EmptyName,
    ReservedName { name: String },
    InvalidName { name: String },
    InvalidValue { name: String },
}

impl std::fmt::Display for EventHeadersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "event header name cannot be empty"),
            Self::ReservedName { name } => write!(f, "event header name '{name}' is reserved"),
            Self::InvalidName { name } => write!(f, "event header name '{name}' is not a valid header name suffix"),
            Self::InvalidValue { name } => write!(f, "event header value for '{name}' is not a valid header value"),
        }
    }
}

impl std::error::Error for EventHeadersError {}
