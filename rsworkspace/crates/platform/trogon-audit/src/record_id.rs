use uuid::Uuid;

use crate::constants::RECORD_ID_UUID_VERSION;
use trogon_std::uuid::NowV7;

/// A record's identity: a UUIDv7 that is also the decision identifier a surface
/// hands back to its caller and the `Nats-Msg-Id` the stream deduplicates on
/// (ADR#0086 Decisions 1 and 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecordIdError {
    #[error("record id must be a UUIDv7")]
    NotUuidV7,
}

impl RecordId {
    pub fn generate(generator: &impl NowV7) -> Self {
        Self(generator.now_v7())
    }

    pub fn from_uuid(uuid: Uuid) -> Result<Self, RecordIdError> {
        if uuid.get_version_num() != RECORD_ID_UUID_VERSION {
            return Err(RecordIdError::NotUuidV7);
        }
        Ok(Self(uuid))
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

#[cfg(test)]
mod tests;
