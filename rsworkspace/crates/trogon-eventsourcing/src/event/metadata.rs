use std::{borrow::Borrow, collections::BTreeMap, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetadataKey(String);

impl MetadataKey {
    pub fn new(value: impl Into<String>) -> Result<Self, EventMetadataError> {
        let value = value.into();
        validate_key(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for MetadataKey {
    type Err = EventMetadataError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for MetadataKey {
    type Error = EventMetadataError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for MetadataKey {
    type Error = EventMetadataError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for MetadataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for MetadataKey {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

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
        validate_value(&key, &value)?;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventMetadataError {
    EmptyName,
    ReservedName { name: String },
    InvalidName { name: String },
    InvalidValue { name: String },
}

impl std::fmt::Display for EventMetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "event metadata name cannot be empty"),
            Self::ReservedName { name } => write!(f, "event metadata name '{name}' is reserved"),
            Self::InvalidName { name } => write!(f, "event metadata name '{name}' is not a valid header name suffix"),
            Self::InvalidValue { name } => write!(f, "event metadata value for '{name}' is not a valid header value"),
        }
    }
}

impl std::error::Error for EventMetadataError {}

#[inline]
fn validate_key(value: &str) -> Result<(), EventMetadataError> {
    if value.is_empty() {
        return Err(EventMetadataError::EmptyName);
    }
    if starts_with_ascii_case_insensitive(value, "nats-") || starts_with_ascii_case_insensitive(value, "trogon-") {
        return Err(EventMetadataError::ReservedName {
            name: value.to_string(),
        });
    }
    if value.contains(|c: char| c == ':' || (c as u8) < 33 || (c as u8) > 126) {
        return Err(EventMetadataError::InvalidName {
            name: value.to_string(),
        });
    }
    Ok(())
}

fn validate_value(key: &MetadataKey, value: &str) -> Result<(), EventMetadataError> {
    if value.contains(['\r', '\n']) {
        return Err(EventMetadataError::InvalidValue {
            name: key.as_str().to_string(),
        });
    }
    Ok(())
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}
