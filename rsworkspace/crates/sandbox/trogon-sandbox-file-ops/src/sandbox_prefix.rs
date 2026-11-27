use std::fmt;
use std::str::FromStr;

use crate::SandboxPrefixError;

/// The absolute path inside the sandbox where the served host tree appears, such as `/workspace`.
///
/// Trailing slashes are dropped, so `/` parses to the empty prefix, which names the root itself.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SandboxPrefix(String);

impl SandboxPrefix {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SandboxPrefixError> {
        let value = value.as_ref();
        if value.contains('\0') {
            return Err(SandboxPrefixError::ContainsNul);
        }
        if !value.starts_with('/') {
            return Err(SandboxPrefixError::NotAbsolute);
        }
        let trimmed = value.trim_end_matches('/');
        if trimmed
            .split('/')
            .skip(1)
            .any(|segment| matches!(segment, "" | "." | ".."))
        {
            return Err(SandboxPrefixError::NotNormalized);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The prefix without a trailing slash; empty when the prefix is the sandbox root.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SandboxPrefix {
    type Err = SandboxPrefixError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for SandboxPrefix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
