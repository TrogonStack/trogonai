use std::fmt;
use std::str::FromStr;

use crate::CommitShaError;
use crate::constants::{MAX_SHA_LEN, MIN_SHA_LEN};

/// A git commit id: hexadecimal, so it can never read as an option to `git checkout`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSha(String);

impl CommitSha {
    pub fn new(value: impl Into<String>) -> Result<Self, CommitShaError> {
        let value = value.into();
        if !(MIN_SHA_LEN..=MAX_SHA_LEN).contains(&value.len()) {
            return Err(CommitShaError::Length {
                min: MIN_SHA_LEN,
                max: MAX_SHA_LEN,
            });
        }
        if !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CommitShaError::NotHex);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommitSha {
    type Err = CommitShaError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
