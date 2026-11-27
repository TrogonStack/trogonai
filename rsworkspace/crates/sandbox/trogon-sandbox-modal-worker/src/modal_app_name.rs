use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::ModalAppNameError;

/// The Modal App every sandbox is created under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ModalAppName(String);

impl ModalAppName {
    pub fn new(value: impl Into<String>) -> Result<Self, ModalAppNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ModalAppNameError::Empty);
        }
        if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(ModalAppNameError::Unsafe);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModalAppName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ModalAppName {
    type Err = ModalAppNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}
