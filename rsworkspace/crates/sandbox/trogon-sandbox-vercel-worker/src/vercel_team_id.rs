use std::str::FromStr;

use crate::VercelTeamIdError;

/// The Vercel team every request is scoped to.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VercelTeamId(String);

impl VercelTeamId {
    pub fn new(value: impl Into<String>) -> Result<Self, VercelTeamIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(VercelTeamIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for VercelTeamId {
    type Err = VercelTeamIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}
