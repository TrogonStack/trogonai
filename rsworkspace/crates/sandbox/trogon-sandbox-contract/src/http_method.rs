use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::HttpMethodError;

/// An HTTP request method as it appears on the request line.
///
/// It is a non-empty token of at most [`HttpMethod::MAX_LEN`] bytes, kept uppercase. It serializes as a JSON string,
/// and construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HttpMethod(String);

impl HttpMethod {
    /// Longest method the wire accepts, in bytes.
    pub const MAX_LEN: usize = 32;

    pub fn new(value: impl Into<String>) -> Result<Self, HttpMethodError> {
        let value = value.into();
        if value.is_empty() {
            return Err(HttpMethodError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(HttpMethodError::TooLong { max: Self::MAX_LEN });
        }
        let is_token = |byte: u8| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
        if !value.bytes().all(is_token) {
            return Err(HttpMethodError::NotAToken);
        }
        Ok(Self(value.to_ascii_uppercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HttpMethod {
    type Error = HttpMethodError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for HttpMethod {
    type Err = HttpMethodError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<HttpMethod> for String {
    fn from(method: HttpMethod) -> Self {
        method.0
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
