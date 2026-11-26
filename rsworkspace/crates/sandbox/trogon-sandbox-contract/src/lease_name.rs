use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::LeaseNameError;

/// The tenant-scoped name a lease can be found by after it stops.
///
/// It holds 1 to [`LeaseName::MAX_LEN`] bytes of any text. Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LeaseName(String);

impl LeaseName {
    /// Longest lease name the wire accepts, in bytes.
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, LeaseNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(LeaseNameError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(LeaseNameError::TooLong { max: Self::MAX_LEN });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LeaseName {
    type Error = LeaseNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for LeaseName {
    type Err = LeaseNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<LeaseName> for String {
    fn from(value: LeaseName) -> Self {
        value.0
    }
}

impl fmt::Display for LeaseName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
