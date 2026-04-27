use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use trogon_std::NowV7;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(Uuid);

impl EventId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub fn now_v7<N>(now_v7: &N) -> Self
    where
        N: NowV7 + ?Sized,
    {
        Self(now_v7.now_v7())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Debug for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EventId").field(&self.0).finish()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for EventId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl From<EventId> for Uuid {
    fn from(value: EventId) -> Self {
        value.0
    }
}

impl FromStr for EventId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self::new)
    }
}

impl Serialize for EventId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(value.as_str()).map_err(serde::de::Error::custom)
    }
}
