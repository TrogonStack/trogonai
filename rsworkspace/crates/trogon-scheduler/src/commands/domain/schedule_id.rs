use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const MAX_LENGTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleId(String);

#[derive(Debug, PartialEq, Eq)]
pub enum ScheduleIdViolation {
    Empty,
    TooLong { max: usize, actual: usize },
    SurroundingWhitespace,
}

#[derive(Debug)]
pub struct ScheduleIdError {
    raw: String,
    violation: ScheduleIdViolation,
}

impl ScheduleId {
    pub fn parse(raw: &str) -> Result<Self, ScheduleIdError> {
        if raw.is_empty() {
            return Err(ScheduleIdError::new(raw, ScheduleIdViolation::Empty));
        }

        if raw.trim() != raw {
            return Err(ScheduleIdError::new(raw, ScheduleIdViolation::SurroundingWhitespace));
        }

        let length = raw.chars().count();
        if length > MAX_LENGTH {
            return Err(ScheduleIdError::new(
                raw,
                ScheduleIdViolation::TooLong {
                    max: MAX_LENGTH,
                    actual: length,
                },
            ));
        }

        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
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
    fn new(raw: &str, violation: ScheduleIdViolation) -> Self {
        Self {
            raw: raw.to_string(),
            violation,
        }
    }
}

impl fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for ScheduleIdViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("must not be empty"),
            Self::TooLong { max, actual } => {
                write!(f, "must be at most {max} characters, got {actual}")
            }
            Self::SurroundingWhitespace => f.write_str("must not have leading or trailing whitespace"),
        }
    }
}

impl fmt::Display for ScheduleIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "schedule id '{}' is invalid: {}", self.raw, self.violation)
    }
}

impl std::error::Error for ScheduleIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        let error = ScheduleId::parse("").unwrap_err();
        assert_eq!(error.violation, ScheduleIdViolation::Empty);
    }

    #[test]
    fn rejects_surrounding_whitespace() {
        for raw in [" backup", "backup ", "\tbackup", "backup\n"] {
            let error = ScheduleId::parse(raw).unwrap_err();
            assert_eq!(error.violation, ScheduleIdViolation::SurroundingWhitespace, "{raw:?}");
        }
    }

    #[test]
    fn accepts_max_length() {
        let raw = "a".repeat(MAX_LENGTH);
        assert_eq!(ScheduleId::parse(&raw).unwrap().as_str(), raw);
    }

    #[test]
    fn rejects_over_max_length() {
        let raw = "a".repeat(MAX_LENGTH + 1);
        let error = ScheduleId::parse(&raw).unwrap_err();
        assert_eq!(
            error.violation,
            ScheduleIdViolation::TooLong {
                max: MAX_LENGTH,
                actual: MAX_LENGTH + 1,
            }
        );
    }

    #[test]
    fn counts_length_in_characters_not_bytes() {
        let raw = "é".repeat(MAX_LENGTH);
        assert_eq!(ScheduleId::parse(&raw).unwrap().as_str(), raw);
    }

    #[test]
    fn accepts_values_a_nats_token_would_reject() {
        for raw in ["report.v2", "orders/created", "ns:thing", "user@host", "a.b.c.d"] {
            assert_eq!(ScheduleId::parse(raw).unwrap().as_str(), raw, "{raw:?}");
        }
    }

    #[test]
    fn accepts_non_ascii() {
        for raw in ["café-nightly", "日次バックアップ", "Pública"] {
            assert_eq!(ScheduleId::parse(raw).unwrap().as_str(), raw, "{raw:?}");
        }
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn accepts_any_non_empty_bounded_value(s in "[a-zA-Z0-9._:@/-]{1,256}") {
                let id = ScheduleId::parse(&s).unwrap();
                prop_assert_eq!(id.as_str(), s.as_str());
            }

            #[test]
            fn serde_round_trips(s in "[a-zA-Z0-9._:@/-]{1,64}") {
                let id = ScheduleId::parse(&s).unwrap();
                let json = serde_json::to_string(&id).unwrap();
                let decoded: ScheduleId = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(decoded, id);
            }
        }
    }
}
