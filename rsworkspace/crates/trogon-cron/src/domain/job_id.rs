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
        None
    }
}
