use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventMetadata {
    entries: BTreeMap<String, String>,
}

impl EventMetadata {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn one(name: impl Into<String>, value: impl Into<String>) -> Result<Self, EventMetadataError> {
        let mut metadata = Self::empty();
        metadata.insert(name, value)?;
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
            metadata.insert(name, value)?;
        }
        Ok(metadata)
    }

    pub fn insert(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Option<String>, EventMetadataError> {
        let name = name.into();
        let value = value.into();
        validate_name(&name)?;
        validate_value(&name, &value)?;
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

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(name, value)| (name.as_str(), value.as_str()))
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

fn validate_name(name: &str) -> Result<(), EventMetadataError> {
    if name.is_empty() {
        return Err(EventMetadataError::EmptyName);
    }
    if starts_with_ascii_case_insensitive(name, "nats-") || starts_with_ascii_case_insensitive(name, "trogon-") {
        return Err(EventMetadataError::ReservedName { name: name.to_string() });
    }
    if name.contains(|c: char| c == ':' || (c as u8) < 33 || (c as u8) > 126) {
        return Err(EventMetadataError::InvalidName { name: name.to_string() });
    }
    Ok(())
}

fn validate_value(name: &str, value: &str) -> Result<(), EventMetadataError> {
    if value.contains(['\r', '\n']) {
        return Err(EventMetadataError::InvalidValue { name: name.to_string() });
    }
    Ok(())
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}
