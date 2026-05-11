use std::collections::BTreeMap;

use super::{event_metadata_error::EventMetadataError, metadata_key::MetadataKey};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventMetadata {
    entries: BTreeMap<MetadataKey, String>,
}

impl EventMetadata {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn one(key: MetadataKey, value: impl Into<String>) -> Result<Self, EventMetadataError> {
        let mut metadata = Self::empty();
        metadata.insert(key, value)?;
        Ok(metadata)
    }

    pub fn from_entries<I, N, V>(entries: I) -> Result<Self, EventMetadataError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let mut metadata = Self::empty();
        for (name, value) in entries {
            metadata.insert(MetadataKey::new(name)?, value)?;
        }
        Ok(metadata)
    }

    pub fn insert(&mut self, key: MetadataKey, value: impl Into<String>) -> Result<Option<String>, EventMetadataError> {
        let value = value.into();
        if value.contains(['\r', '\n']) {
            return Err(EventMetadataError::InvalidValue {
                name: key.as_str().to_string(),
            });
        }
        Ok(self.entries.insert(key, value))
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

    pub fn iter(&self) -> impl Iterator<Item = (&MetadataKey, &str)> {
        self.entries.iter().map(|(key, value)| (key, value.as_str()))
    }
}
