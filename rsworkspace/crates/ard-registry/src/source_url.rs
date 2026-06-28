//! Validated registry source URL value object.

use std::sync::Arc;

/// Error returned when [`SourceUrl`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceUrlError {
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),
}

/// A validated URL that identifies the registry's own HTTP origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceUrl(Arc<str>);

impl SourceUrl {
    pub fn parse(value: &str) -> Result<Self, SourceUrlError> {
        url::Url::parse(value)?;
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
