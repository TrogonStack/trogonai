use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::commands::domain::{ScheduleId as DomainScheduleId, ScheduleIdError as DomainScheduleIdError};

/// A schedule identifier for read-model queries.
///
/// This shares the command domain's identifier contract so any schedule that can
/// be created can also be queried: the read model derives a token-safe KV key
/// from the raw id, so ids the underlying NATS KV key syntax would reject (dots,
/// slashes, `:`, `@`, non-ASCII) are still addressable here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScheduleId(String);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ScheduleIdError(#[source] DomainScheduleIdError);

impl ScheduleId {
    pub fn parse(raw: &str) -> Result<Self, ScheduleIdError> {
        DomainScheduleId::parse(raw)
            .map(|id| Self(id.as_str().to_string()))
            .map_err(ScheduleIdError)
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

impl fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
