use std::fmt;
use std::str::FromStr;

use crate::sublayer_guid_error::SublayerGuidError;

/// GUID of the WFP sublayer holding the fence filters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SublayerGuid(String);

impl SublayerGuid {
    pub fn new(value: impl Into<String>) -> Result<Self, SublayerGuidError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SublayerGuidError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SublayerGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SublayerGuid {
    type Err = SublayerGuidError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}
