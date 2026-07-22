use serde::{Deserialize, Serialize};

use crate::error::JwtError;

/// NATS-safe caller identifier carried in `UserJwtClaims.caller_id` and used as
/// a single subject segment in audit/DLQ paths. Single segment: no `.`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CallerId(String);

impl CallerId {
    pub fn new(segment: impl Into<String>) -> Result<Self, JwtError> {
        let s = segment.into();
        validate_caller_segment(&s).map(|()| Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CallerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

fn validate_caller_segment(s: &str) -> Result<(), JwtError> {
    if s.is_empty() {
        return Err(JwtError::InvalidCallerId);
    }
    for c in s.chars() {
        match c {
            '.' | '*' | '>' => return Err(JwtError::InvalidCallerId),
            c if c.is_whitespace() => return Err(JwtError::InvalidCallerId),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
