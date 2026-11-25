use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ForwardUrlError;

/// The URL a [`RequestForward`](crate::RequestForward) sends matched requests to.
///
/// It parses as a URL and carries no query string and no fragment, an empty one included. The text is kept as
/// written, so it serializes unchanged.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ForwardUrl(String);

impl ForwardUrl {
    pub fn new(value: impl Into<String>) -> Result<Self, ForwardUrlError> {
        let value = value.into();
        let parsed = url::Url::parse(&value).map_err(|source| ForwardUrlError::Invalid {
            input: value.clone(),
            source,
        })?;
        if parsed.query().is_some() {
            return Err(ForwardUrlError::HasQuery(value));
        }
        if parsed.fragment().is_some() {
            return Err(ForwardUrlError::HasFragment(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ForwardUrl {
    type Error = ForwardUrlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ForwardUrl {
    type Err = ForwardUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ForwardUrl> for String {
    fn from(url: ForwardUrl) -> Self {
        url.0
    }
}

impl fmt::Display for ForwardUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
