use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ExtraCapabilityError;

/// A Linux capability a lease keeps in addition to the worker's retained set, without the `CAP_` prefix.
///
/// A name is one or more upper case letters, digits, or `_`, and never one of [`ExtraCapability::FORBIDDEN`]. It
/// serializes as a JSON string, and construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExtraCapability(String);

impl ExtraCapability {
    /// Capabilities no override may add back, whatever the tenant policy says. They give the payload a way to
    /// leave the container or to rewrite the host.
    pub const FORBIDDEN: [&'static str; 8] = [
        "ALL",
        "SYS_ADMIN",
        "SYS_MODULE",
        "SYS_RAWIO",
        "SYS_BOOT",
        "SYS_TIME",
        "MAC_ADMIN",
        "MAC_OVERRIDE",
    ];

    pub fn new(value: impl Into<String>) -> Result<Self, ExtraCapabilityError> {
        let name = value.into();
        let well_formed = !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if !well_formed {
            return Err(ExtraCapabilityError::InvalidName(name));
        }
        if Self::FORBIDDEN.contains(&name.as_str()) {
            return Err(ExtraCapabilityError::Forbidden(name));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExtraCapability {
    type Error = ExtraCapabilityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ExtraCapability {
    type Err = ExtraCapabilityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ExtraCapability> for String {
    fn from(capability: ExtraCapability) -> Self {
        capability.0
    }
}

impl fmt::Display for ExtraCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
