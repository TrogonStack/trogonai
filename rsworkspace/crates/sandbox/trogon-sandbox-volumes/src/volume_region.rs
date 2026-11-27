use std::fmt;
use std::str::FromStr;

use crate::VolumeRegionError;

/// A cloud region name: a non-empty token without whitespace or control characters.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct VolumeRegion(String);

impl VolumeRegion {
    pub fn new(value: impl AsRef<str>) -> Result<Self, VolumeRegionError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(VolumeRegionError::Empty);
        }
        if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(VolumeRegionError::InvalidCharacter);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for VolumeRegion {
    type Err = VolumeRegionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for VolumeRegion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
