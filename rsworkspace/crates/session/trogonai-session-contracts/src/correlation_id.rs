use crate::identifier::{validate_prefixed_identifier, IdentifierError};

/// Correlation identifier grouping related events (`corr_...`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CorrelationId(String);

impl CorrelationId {
    pub const PREFIX: &'static str = "corr_";

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_prefixed_identifier(&value, Self::PREFIX)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for CorrelationId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&str> for CorrelationId {
    type Error = IdentifierError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
