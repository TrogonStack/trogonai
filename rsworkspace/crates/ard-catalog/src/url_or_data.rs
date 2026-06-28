//! ARD catalog entry delivery mode: exactly one of URL or embedded data.

use serde_json::Value;
use std::sync::Arc;

use url::Url;

/// Error returned when [`UrlOrData`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlOrDataError {
    #[error("catalog entry must contain exactly one of url or data")]
    BothUrlAndData,
    #[error("catalog entry must contain url or data")]
    NeitherUrlNorData,
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("data must be a JSON object")]
    DataMustBeObject,
}

/// Enforces ARD value-or-reference delivery in the type system.
#[derive(Clone, Debug, PartialEq)]
pub enum UrlOrData {
    Url(Arc<str>),
    Data(Value),
}

impl UrlOrData {
    pub fn from_optional(url: Option<String>, data: Option<Value>) -> Result<Self, UrlOrDataError> {
        match (url, data) {
            (Some(_), Some(_)) => Err(UrlOrDataError::BothUrlAndData),
            (None, None) => Err(UrlOrDataError::NeitherUrlNorData),
            (Some(url), None) => {
                Url::parse(&url)?;
                Ok(Self::Url(Arc::from(url)))
            }
            (None, Some(data)) if data.is_object() => Ok(Self::Data(data)),
            (None, Some(_)) => Err(UrlOrDataError::DataMustBeObject),
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url(url) => Some(url),
            Self::Data(_) => None,
        }
    }

    pub fn data(&self) -> Option<&Value> {
        match self {
            Self::Url(_) => None,
            Self::Data(data) => Some(data),
        }
    }
}

#[cfg(test)]
mod tests;
