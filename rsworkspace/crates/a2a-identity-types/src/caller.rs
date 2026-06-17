use serde::{Deserialize, Serialize};

use crate::error::JwtError;

/// NATS-safe caller identifier carried in `UserJwtClaims.caller_id` and used as
/// a single subject segment in audit/DLQ paths. Single segment: no `.`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

fn validate_caller_segment(s: &str) -> Result<(), JwtError> {
    if s.is_empty() || s.contains('.') {
        return Err(JwtError::InvalidCallerId);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_segment() {
        let caller = CallerId::new("alice").unwrap();
        assert_eq!(caller.as_str(), "alice");
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(CallerId::new(""), Err(JwtError::InvalidCallerId)));
    }

    #[test]
    fn rejects_dotted() {
        assert!(matches!(CallerId::new("a.b"), Err(JwtError::InvalidCallerId)));
    }

    #[test]
    fn serializes_transparent() {
        let caller = CallerId::new("alice").unwrap();
        let json = serde_json::to_string(&caller).unwrap();
        assert_eq!(json, "\"alice\"");
    }

    #[test]
    fn deserializes_transparent() {
        let caller: CallerId = serde_json::from_str("\"alice\"").unwrap();
        assert_eq!(caller.as_str(), "alice");
    }
}
