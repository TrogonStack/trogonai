use std::{borrow::Borrow, str::FromStr};

use super::event_headers_error::EventHeadersError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeaderName(String);

impl HeaderName {
    pub fn new(name: impl Into<String>) -> Result<Self, EventHeadersError> {
        let name = name.into();
        if name.is_empty() {
            return Err(EventHeadersError::EmptyName);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for HeaderName {
    type Err = EventHeadersError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for HeaderName {
    type Error = EventHeadersError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for HeaderName {
    type Error = EventHeadersError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for HeaderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for HeaderName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}
