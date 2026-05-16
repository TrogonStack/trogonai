use std::{borrow::Borrow, str::FromStr};

use super::event_headers_error::EventHeadersError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeaderKey(String);

impl HeaderKey {
    pub fn new(value: impl Into<String>) -> Result<Self, EventHeadersError> {
        let value = value.into();
        if value.is_empty() {
            return Err(EventHeadersError::EmptyName);
        }
        if starts_with_ascii_case_insensitive(&value, "nats-") || starts_with_ascii_case_insensitive(&value, "trogon-")
        {
            return Err(EventHeadersError::ReservedName { name: value });
        }
        if value.contains(|c: char| c == ':' || (c as u8) < 33 || (c as u8) > 126) {
            return Err(EventHeadersError::InvalidName { name: value });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for HeaderKey {
    type Err = EventHeadersError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for HeaderKey {
    type Error = EventHeadersError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for HeaderKey {
    type Error = EventHeadersError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for HeaderKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for HeaderKey {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}
