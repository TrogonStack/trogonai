use std::fmt;

use serde::{Deserialize, Serialize};

use crate::MachineIdError;

/// A Fly Machine id: 1 to 64 ASCII letters, digits, or `-`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MachineId(String);

impl MachineId {
    pub fn new(value: impl Into<String>) -> Result<Self, MachineIdError> {
        let value = value.into();
        let well_formed = !value.is_empty()
            && value.len() <= 64
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if well_formed {
            Ok(Self(value))
        } else {
            Err(MachineIdError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MachineId {
    type Error = MachineIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MachineId> for String {
    fn from(value: MachineId) -> Self {
        value.0
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
