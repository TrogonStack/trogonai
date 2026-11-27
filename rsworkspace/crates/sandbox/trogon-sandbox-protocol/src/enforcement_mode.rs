use std::str::FromStr;

use crate::EnforcementModeError;

/// How a worker treats a dispatch whose signed envelope is missing or invalid.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EnforcementMode {
    /// Skip verification entirely.
    Off,
    /// Log a missing or invalid envelope and accept the dispatch.
    Warn,
    /// Refuse a missing or invalid envelope.
    Require,
}

impl EnforcementMode {
    /// Parses `off`, `warn`, or `require`, ignoring case and surrounding space. An empty value means `warn`.
    pub fn parse(value: &str) -> Result<Self, EnforcementModeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "warn" | "" => Ok(Self::Warn),
            "require" => Ok(Self::Require),
            other => Err(EnforcementModeError {
                found: other.to_owned(),
            }),
        }
    }
}

impl FromStr for EnforcementMode {
    type Err = EnforcementModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
