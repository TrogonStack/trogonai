use std::fmt;

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
