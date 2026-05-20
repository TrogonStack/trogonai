use std::collections::BTreeMap;

use super::{
    from_entries_error::FromEntriesError,
    header_name::HeaderName,
    header_value::{HeaderValue, HeaderValueError},
};

/// Deterministic event metadata map.
///
/// A header name has at most one value. The `BTreeMap` backing keeps iteration
/// deterministic for tests, signatures, and adapters that need stable ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    entries: BTreeMap<HeaderName, HeaderValue>,
}

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty header map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty header map.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Creates a header map containing one validated entry.
    pub fn one<V>(name: HeaderName, value: V) -> Result<Self, HeaderValueError>
    where
        V: TryInto<HeaderValue>,
        HeaderValueError: From<V::Error>,
    {
        let mut headers = Self::empty();
        headers.insert(name, value)?;
        Ok(headers)
    }

    /// Builds a header map from raw name/value pairs.
    ///
    /// Use this at input boundaries where names and values have not yet been
    /// converted into domain value objects.
    pub fn from_entries<I, N, V>(entries: I) -> Result<Self, FromEntriesError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: TryInto<HeaderValue>,
        HeaderValueError: From<V::Error>,
    {
        let mut headers = Self::empty();
        for (name, value) in entries {
            let name = name.into();
            let header_name = HeaderName::new(name.clone()).map_err(|source| FromEntriesError::InvalidName {
                name: name.clone(),
                source,
            })?;
            headers
                .insert(header_name, value)
                .map_err(|source| FromEntriesError::InvalidValue { name, source })?;
        }
        Ok(headers)
    }

    /// Inserts or replaces one header value.
    pub fn insert<V>(&mut self, name: HeaderName, value: V) -> Result<Option<HeaderValue>, HeaderValueError>
    where
        V: TryInto<HeaderValue>,
        HeaderValueError: From<V::Error>,
    {
        let value = value.try_into().map_err(HeaderValueError::from)?;
        Ok(self.entries.insert(name, value))
    }

    /// Looks up a header by its exact name.
    pub fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.entries.get(name)
    }

    /// Looks up a header and returns its string value.
    pub fn get_str(&self, name: &str) -> Option<&str> {
        self.get(name).map(HeaderValue::as_str)
    }

    /// Returns `true` when no headers are present.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of header entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Iterates over headers in deterministic name order.
    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.entries.iter()
    }
}
