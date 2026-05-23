use std::fmt;

use a2a_redaction::RedactionError;

#[derive(Debug)]
pub enum PolicyError {
    Redaction(RedactionError),
    Tier2(Tier2EvalError),
}

#[derive(Debug)]
pub struct Tier2EvalError(pub(crate) Box<str>);

impl fmt::Display for Tier2EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Tier2EvalError {}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Redaction(inner) => write!(f, "{inner}"),
            Self::Tier2(inner) => write!(f, "{inner}"),
        }
    }
}

impl std::error::Error for PolicyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Redaction(inner) => Some(inner),
            Self::Tier2(inner) => Some(inner),
        }
    }
}

impl From<RedactionError> for PolicyError {
    fn from(value: RedactionError) -> Self {
        Self::Redaction(value)
    }
}

impl From<Tier2EvalError> for PolicyError {
    fn from(value: Tier2EvalError) -> Self {
        Self::Tier2(value)
    }
}
