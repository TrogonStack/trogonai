use std::str::FromStr;

use serde::Serialize;

use crate::VercelProjectIdError;

/// The Vercel project sandboxes are created in.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct VercelProjectId(String);

impl VercelProjectId {
    pub fn new(value: impl Into<String>) -> Result<Self, VercelProjectIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(VercelProjectIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for VercelProjectId {
    type Err = VercelProjectIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}
