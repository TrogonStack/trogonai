use std::fmt;

use crate::FlyAppNameError;

/// The name of a Fly app: 1 to 63 ASCII letters, digits, or `-`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct FlyAppName(String);

impl FlyAppName {
    pub fn new(value: impl Into<String>) -> Result<Self, FlyAppNameError> {
        let value = value.into();
        let well_formed = !value.is_empty()
            && value.len() <= 63
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if well_formed {
            Ok(Self(value))
        } else {
            Err(FlyAppNameError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FlyAppName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
