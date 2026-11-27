use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ActorIdError;

/// The identity the runtime assigned to this actor: a non-empty string with no surrounding whitespace.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActorId(String);

impl ActorId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, ActorIdError> {
        let trimmed = value.as_ref().trim();
        if trimmed.is_empty() {
            return Err(ActorIdError::Empty);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ActorId {
    type Error = ActorIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ActorId> for String {
    fn from(value: ActorId) -> Self {
        value.0
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
