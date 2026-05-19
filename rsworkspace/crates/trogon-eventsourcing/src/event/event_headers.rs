use std::collections::BTreeMap;

use super::{event_headers_error::EventHeadersError, header_name::HeaderName};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventHeaders {
    entries: BTreeMap<HeaderName, String>,
}

impl EventHeaders {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn one(name: HeaderName, value: impl Into<String>) -> Result<Self, EventHeadersError> {
        let mut headers = Self::empty();
        headers.insert(name, value)?;
        Ok(headers)
    }

    pub fn from_entries<I, N, V>(entries: I) -> Result<Self, EventHeadersError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let mut headers = Self::empty();
        for (name, value) in entries {
            headers.insert(HeaderName::new(name)?, value)?;
        }
        Ok(headers)
    }

    pub fn insert(&mut self, name: HeaderName, value: impl Into<String>) -> Result<Option<String>, EventHeadersError> {
        let value = value.into();
        if value.contains(['\r', '\n']) {
            return Err(EventHeadersError::InvalidValue {
                name: name.as_str().to_string(),
            });
        }
        Ok(self.entries.insert(name, value))
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &str)> {
        self.entries.iter().map(|(name, value)| (name, value.as_str()))
    }
}
