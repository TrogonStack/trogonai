use std::{convert::Infallible, str::FromStr};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderValue(String);

impl HeaderValue {
    pub fn new(value: impl Into<String>) -> Result<Self, HeaderValueError> {
        let value = value.into();
        if value.contains(['\r', '\n', '\0']) {
            return Err(HeaderValueError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl FromStr for HeaderValue {
    type Err = HeaderValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for HeaderValue {
    type Error = HeaderValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for HeaderValue {
    type Error = HeaderValueError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&String> for HeaderValue {
    type Error = HeaderValueError;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::new(value.as_str())
    }
}

impl std::fmt::Display for HeaderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl AsRef<str> for HeaderValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValueError;

impl std::fmt::Display for HeaderValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("event header value cannot contain '\\r', '\\n', or '\\0'")
    }
}

impl std::error::Error for HeaderValueError {}

impl From<Infallible> for HeaderValueError {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}
