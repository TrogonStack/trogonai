use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use trogon_nats::{NatsToken, SubjectTokenViolation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobId(NatsToken);

#[derive(Debug)]
pub struct JobIdError {
    raw: String,
    source: SubjectTokenViolation,
}

impl JobId {
    pub fn parse(raw: &str) -> Result<Self, JobIdError> {
        NatsToken::new(raw)
            .map(Self)
            .map_err(|source| JobIdError::new(raw, source))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn as_token(&self) -> &NatsToken {
        &self.0
    }
}

impl AsRef<str> for JobId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for JobId {
    type Err = JobIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for JobId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl JobIdError {
    fn new(raw: &str, source: SubjectTokenViolation) -> Self {
        Self {
            raw: raw.to_string(),
            source,
        }
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for JobIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job id '{}' is invalid: {:?}", self.raw, self.source)
    }
}

impl std::error::Error for JobIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_job_id_preserves_subject_token_violation_as_source() {
        let error = JobId::parse("").unwrap_err();

        let source = std::error::Error::source(&error).unwrap();

        assert_eq!(source.to_string(), SubjectTokenViolation::Empty.to_string());
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn job_id_accepts_any_valid_single_token(s in "[a-z0-9_-]{1,128}") {
                let id = JobId::parse(&s).unwrap();
                prop_assert_eq!(id.as_str(), s.as_str());
            }

            #[test]
            fn job_id_serde_round_trips(s in "[a-z0-9_-]{1,64}") {
                let id = JobId::parse(&s).unwrap();
                let json = serde_json::to_string(&id).unwrap();
                let decoded: JobId = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(decoded, id);
            }

            #[test]
            fn job_id_rejects_string_containing_dot(
                prefix in "[a-z]{1,8}",
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}.{suffix}");
                prop_assert!(JobId::parse(&s).is_err());
            }

            #[test]
            fn job_id_rejects_string_containing_wildcard(
                prefix in "[a-z]{1,8}",
                wildcard in prop_oneof![Just('*'), Just('>')],
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}{wildcard}{suffix}");
                prop_assert!(JobId::parse(&s).is_err());
            }

            #[test]
            fn job_id_rejects_string_containing_whitespace(
                prefix in "[a-z]{1,8}",
                ws in prop_oneof![Just(' '), Just('\t'), Just('\n'), Just('\r')],
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}{ws}{suffix}");
                prop_assert!(JobId::parse(&s).is_err());
            }

            #[test]
            fn job_id_rejects_non_ascii(
                prefix in "[a-z]{0,5}",
                non_ascii in "[^\x00-\x7F]{1,3}",
                suffix in "[a-z]{0,5}",
            ) {
                let s = format!("{prefix}{non_ascii}{suffix}");
                prop_assert!(JobId::parse(&s).is_err());
            }

            #[test]
            fn job_id_rejects_string_over_max_length(n in 129usize..200usize) {
                let s = "a".repeat(n);
                prop_assert!(JobId::parse(&s).is_err());
            }
        }
    }
}
