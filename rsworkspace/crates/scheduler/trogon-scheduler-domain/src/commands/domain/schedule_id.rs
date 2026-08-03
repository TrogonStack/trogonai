use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScheduleId(Uuid);

#[derive(Debug, thiserror::Error)]
#[error("schedule id '{raw}' is invalid: must be a UUID")]
pub struct ScheduleIdError {
    raw: String,
    #[source]
    source: uuid::Error,
}

impl ScheduleId {
    pub fn parse(raw: &str) -> Result<Self, ScheduleIdError> {
        let uuid = Uuid::parse_str(raw).map_err(|source| ScheduleIdError::new(raw, source))?;
        Ok(Self(uuid))
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl FromStr for ScheduleId {
    type Err = ScheduleIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl From<Uuid> for ScheduleId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl ScheduleIdError {
    fn new(raw: &str, source: uuid::Error) -> Self {
        Self {
            raw: raw.to_string(),
            source,
        }
    }
}

impl fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_simple())
    }
}

#[cfg(test)]
mod tests;
