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
    fn rejects_nats_wildcards() {
        assert!(matches!(CallerId::new("*"), Err(JwtError::InvalidCallerId)));
        assert!(matches!(CallerId::new(">"), Err(JwtError::InvalidCallerId)));
        assert!(matches!(CallerId::new("a*b"), Err(JwtError::InvalidCallerId)));
        assert!(matches!(CallerId::new("a>b"), Err(JwtError::InvalidCallerId)));
    }

    #[test]
    fn rejects_whitespace() {
        assert!(matches!(CallerId::new("a b"), Err(JwtError::InvalidCallerId)));
        assert!(matches!(CallerId::new("a\tb"), Err(JwtError::InvalidCallerId)));
    }

    #[test]
    fn serializes_transparent() {
        let caller = CallerId::new("alice").unwrap();
        let json = serde_json::to_string(&caller).unwrap();
        assert_eq!(json, "\"alice\"");
    }

    #[test]
    fn deserializes_transparent_when_valid() {
        let caller: CallerId = serde_json::from_str("\"alice\"").unwrap();
        assert_eq!(caller.as_str(), "alice");
    }

    #[test]
    fn deserialize_rejects_dotted_input() {
        let err = serde_json::from_str::<CallerId>("\"a.b\"").unwrap_err();
        assert!(err.to_string().contains("caller_id"));
    }

    #[test]
    fn deserialize_rejects_empty_input() {
        let err = serde_json::from_str::<CallerId>("\"\"").unwrap_err();
        assert!(err.to_string().contains("caller_id"));
    }
}
