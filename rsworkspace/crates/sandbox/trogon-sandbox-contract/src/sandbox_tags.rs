use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::SandboxTagsError;

/// Caller-defined key-value metadata attached to a lease, such as an environment or a team.
///
/// It serializes as a JSON object of strings. Construction and deserialization reject more than
/// [`SandboxTags::MAX_TAGS`] entries, so a malformed request fails at the boundary.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BTreeMap<String, String>", into = "BTreeMap<String, String>")]
pub struct SandboxTags(BTreeMap<String, String>);

impl SandboxTags {
    /// Maximum number of tags a single lease may carry.
    pub const MAX_TAGS: usize = 5;

    pub fn new(tags: BTreeMap<String, String>) -> Result<Self, SandboxTagsError> {
        if tags.len() > Self::MAX_TAGS {
            return Err(SandboxTagsError::TooMany {
                actual: tags.len(),
                max: Self::MAX_TAGS,
            });
        }
        Ok(Self(tags))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Whether every `(key, value)` in `filter` is present. An empty filter matches every tag set.
    pub fn contains_all(&self, filter: &BTreeMap<String, String>) -> bool {
        filter.iter().all(|(key, value)| self.0.get(key) == Some(value))
    }
}

impl TryFrom<BTreeMap<String, String>> for SandboxTags {
    type Error = SandboxTagsError;

    fn try_from(tags: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        Self::new(tags)
    }
}

impl From<SandboxTags> for BTreeMap<String, String> {
    fn from(tags: SandboxTags) -> Self {
        tags.0
    }
}
