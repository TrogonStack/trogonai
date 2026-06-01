use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use trogon_nats::{NatsToken, SubjectTokenViolation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleId(NatsToken);

#[derive(Debug)]
pub struct ScheduleIdError {
    raw: String,
    source: SubjectTokenViolation,
}

impl ScheduleId {
    pub fn parse(raw: &str) -> Result<Self, ScheduleIdError> {
        NatsToken::new(raw)
            .map(Self)
            .map_err(|source| ScheduleIdError::new(raw, source))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn as_token(&self) -> &NatsToken {
        &self.0
    }
}

impl AsRef<str> for ScheduleId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ScheduleId {
    type Err = ScheduleIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for ScheduleId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ScheduleId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl ScheduleIdError {
    fn new(raw: &str, source: SubjectTokenViolation) -> Self {
        Self {
            raw: raw.to_string(),
            source,
        }
    }
}

impl fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for ScheduleIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "schedule id '{}' is invalid: {}", self.raw, self.source)
    }
}

impl std::error::Error for ScheduleIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_schedule_id_preserves_subject_token_violation_as_source() {
        let error = ScheduleId::parse("").unwrap_err();

        let source = std::error::Error::source(&error).unwrap();

        assert_eq!(source.to_string(), SubjectTokenViolation::Empty.to_string());
    }
}
