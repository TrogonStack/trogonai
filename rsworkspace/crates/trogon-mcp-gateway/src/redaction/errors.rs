use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionError {
    InvalidPath(String),
    InvalidYaml(String),
    UnknownAction(String),
}

impl fmt::Display for RedactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(msg) => write!(f, "invalid json path: {msg}"),
            Self::InvalidYaml(msg) => write!(f, "invalid yaml: {msg}"),
            Self::UnknownAction(key) => write!(f, "unknown redaction action: {key}"),
        }
    }
}

impl std::error::Error for RedactionError {}
