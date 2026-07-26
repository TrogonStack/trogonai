use std::fmt;
use std::str::FromStr;

const MAX_LENGTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleId(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleIdViolationError {
    #[error("must not be empty")]
    Empty,
    #[error("must be at most {max} characters, got {actual}")]
    TooLong { max: usize, actual: usize },
    #[error("must not have leading or trailing whitespace")]
    SurroundingWhitespace,
}

#[derive(Debug, thiserror::Error)]
#[error("schedule id '{raw}' is invalid: {violation}")]
pub struct ScheduleIdError {
    raw: String,
    violation: ScheduleIdViolationError,
}

impl ScheduleId {
    pub fn parse(raw: &str) -> Result<Self, ScheduleIdError> {
        if raw.is_empty() {
            return Err(ScheduleIdError::new(raw, ScheduleIdViolationError::Empty));
        }

        if raw.trim() != raw {
            return Err(ScheduleIdError::new(
                raw,
                ScheduleIdViolationError::SurroundingWhitespace,
            ));
        }

        let length = raw.chars().count();
        if length > MAX_LENGTH {
            return Err(ScheduleIdError::new(
                raw,
                ScheduleIdViolationError::TooLong {
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

impl ScheduleIdError {
    fn new(raw: &str, violation: ScheduleIdViolationError) -> Self {
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

#[cfg(test)]
mod tests;
