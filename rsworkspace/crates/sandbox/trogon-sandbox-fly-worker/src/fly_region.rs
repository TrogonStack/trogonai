use std::fmt;

use crate::FlyRegionError;

/// A Fly region code such as `iad`: letters, digits, `_`, or `-`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct FlyRegion(String);

impl FlyRegion {
    pub fn new(value: impl Into<String>) -> Result<Self, FlyRegionError> {
        let value = value.into();
        let well_formed = !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if well_formed {
            Ok(Self(value))
        } else {
            Err(FlyRegionError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FlyRegion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
