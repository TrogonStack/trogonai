//! Validated registry source URL value object.

use std::sync::Arc;

/// Error returned when [`SourceUrl`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceUrlError {
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("source url scheme must be http or https")]
    UnsupportedScheme(String),
    #[error("source url must be an http/https origin (no path, query, or fragment)")]
    NotAnOrigin,
}

/// A validated URL that identifies the registry's own HTTP origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceUrl(Arc<str>);

impl SourceUrl {
    pub fn parse(value: &str) -> Result<Self, SourceUrlError> {
        let url = url::Url::parse(value)?;
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(SourceUrlError::UnsupportedScheme(other.to_owned())),
        }
        if url.host().is_none() {
            return Err(SourceUrlError::NotAnOrigin);
        }
        let path = url.path();
        if !path.is_empty() && path != "/" {
            return Err(SourceUrlError::NotAnOrigin);
        }
        if url.query().is_some() {
            return Err(SourceUrlError::NotAnOrigin);
        }
        if url.fragment().is_some() {
            return Err(SourceUrlError::NotAnOrigin);
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
