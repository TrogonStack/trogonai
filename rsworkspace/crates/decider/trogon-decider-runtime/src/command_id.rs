use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::EventId;

/// Stable identity for one command, carried across every delivery of that command.
///
/// Exists so a redelivered command produces the same [`EventId`]s as its first attempt. Storage
/// adapters key their deduplication on event identity (`trogon-decider-nats` publishes
/// `event.id` as the JetStream `Nats-Msg-Id`), so a freshly generated id per attempt makes that
/// deduplication window unable to recognize a retry: an at-least-once consumer that redelivers
/// after an ack timeout appends the same events a second time under new ids.
///
/// The id must come from the delivery that carries the command, not from the execution: a value
/// this crate generated would be as fresh on the retry as the UUIDv7 it replaces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(Uuid);

impl CommandId {
    /// Wraps an already assigned command UUID.
    pub const fn new(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the underlying UUID for transports and external APIs.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }

    /// Derives an id from a business key that identifies the command within `namespace`.
    ///
    /// For deliveries that carry no id of their own but do carry a key the sender cannot vary
    /// between attempts, such as a timer wakeup named by the occurrence it fires for. Callers
    /// owning distinct kinds of key must not share a namespace, or two unrelated commands whose
    /// keys happen to render the same way collapse onto one identity.
    pub fn derive(namespace: &Uuid, key: &[u8]) -> Self {
        Self(Uuid::new_v5(namespace, key))
    }

    /// Derives the identity of the event this command decided at `index`.
    ///
    /// A UUIDv5 over this command's id and the event's position in the decided batch, so the same
    /// command decided again yields byte-identical ids in the same order. The index is hashed in
    /// big-endian form so the derivation does not vary by host architecture.
    pub fn event_id(self, index: usize) -> EventId {
        EventId::new(Uuid::new_v5(&self.0, &(index as u64).to_be_bytes()))
    }
}

impl fmt::Debug for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CommandId").field(&self.0).finish()
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for CommandId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl From<CommandId> for Uuid {
    fn from(value: CommandId) -> Self {
        value.0
    }
}

impl FromStr for CommandId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self::new)
    }
}

impl Serialize for CommandId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CommandId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(value.as_str()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
