use std::collections::BTreeMap;

use super::{
    event_headers_from_entries_error::EventHeadersFromEntriesError,
    header_name::HeaderName,
    header_value::{HeaderValue, HeaderValueError},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventHeaders {
    entries: BTreeMap<HeaderName, HeaderValue>,
}

impl EventHeaders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn one<V>(name: HeaderName, value: V) -> Result<Self, HeaderValueError>
    where
        V: TryInto<HeaderValue>,
        HeaderValueError: From<V::Error>,
    {
        let mut headers = Self::empty();
        headers.insert(name, value)?;
        Ok(headers)
    }

    pub fn from_entries<I, N, V>(entries: I) -> Result<Self, EventHeadersFromEntriesError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: TryInto<HeaderValue>,
        HeaderValueError: From<V::Error>,
    {
        let mut headers = Self::empty();
        for (name, value) in entries {
            let name = name.into();
            let header_name =
                HeaderName::new(name.clone()).map_err(|source| EventHeadersFromEntriesError::InvalidName {
                    name: name.clone(),
                    source,
                })?;
            headers
                .insert(header_name, value)
                .map_err(|source| EventHeadersFromEntriesError::InvalidValue { name, source })?;
        }
        Ok(headers)
    }

    pub fn insert<V>(&mut self, name: HeaderName, value: V) -> Result<Option<HeaderValue>, HeaderValueError>
    where
        V: TryInto<HeaderValue>,
        HeaderValueError: From<V::Error>,
    {
        let value = value.try_into().map_err(HeaderValueError::from)?;
        Ok(self.entries.insert(name, value))
    }

    pub fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.entries.get(name)
    }

    pub fn get_str(&self, name: &str) -> Option<&str> {
        self.get(name).map(HeaderValue::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.entries.iter()
    }
}
