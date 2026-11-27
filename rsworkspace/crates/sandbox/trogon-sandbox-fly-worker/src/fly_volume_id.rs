use std::fmt;

use serde::{Deserialize, Serialize};

use crate::FlyVolumeIdError;

/// A Fly volume id: `vol_` followed by letters, digits, or `_`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FlyVolumeId(String);

impl FlyVolumeId {
    pub fn new(value: impl Into<String>) -> Result<Self, FlyVolumeIdError> {
        let value = value.into();
        let well_formed = value.len() <= 256
            && value.starts_with("vol_")
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if well_formed {
            Ok(Self(value))
        } else {
            Err(FlyVolumeIdError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for FlyVolumeId {
    type Error = FlyVolumeIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<FlyVolumeId> for String {
    fn from(value: FlyVolumeId) -> Self {
        value.0
    }
}

impl fmt::Display for FlyVolumeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
