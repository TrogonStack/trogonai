use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::MachServiceNameError;

/// A Mach service the payload may look up.
///
/// A trailing `*` makes it a prefix, and a bare `*` stands for every service. A `*` anywhere else, an empty name,
/// and a control character are refused. It serializes as a JSON string, and construction, parsing, and
/// deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MachServiceName(String);

impl MachServiceName {
    pub fn new(value: impl Into<String>) -> Result<Self, MachServiceNameError> {
        let raw = value.into();
        if raw.is_empty() {
            return Err(MachServiceNameError::Empty);
        }
        if let Some(control) = raw.chars().find(|character| character.is_control()) {
            return Err(MachServiceNameError::ControlCharacter(control));
        }
        let stem = raw.strip_suffix('*').unwrap_or(&raw);
        if stem.contains('*') {
            return Err(MachServiceNameError::InvalidWildcard(raw));
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MachServiceName {
    type Error = MachServiceNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for MachServiceName {
    type Err = MachServiceNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<MachServiceName> for String {
    fn from(name: MachServiceName) -> Self {
        name.0
    }
}

impl fmt::Display for MachServiceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
