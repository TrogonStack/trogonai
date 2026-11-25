use uuid::Uuid;

use crate::constants::CHECKPOINT_ID_UUID_VERSION;
use trogon_std::uuid::NowV7;

/// A checkpoint's identity: a UUIDv7 that is also the `Nats-Msg-Id` the
/// checkpoint stream deduplicates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CheckpointId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointIdError {
    #[error("checkpoint id must be a UUIDv7")]
    NotUuidV7,
}

impl CheckpointId {
    pub fn generate(generator: &impl NowV7) -> Self {
        Self(generator.now_v7())
    }

    /// Reads the hyphenated lowercase form the writer signs, so one checkpoint
    /// has one spelling.
    pub fn parse(value: &str) -> Result<Self, CheckpointIdError> {
        let uuid = Uuid::try_parse(value).map_err(|_| CheckpointIdError::NotUuidV7)?;
        if uuid.get_version_num() != CHECKPOINT_ID_UUID_VERSION || uuid.hyphenated().to_string() != value {
            return Err(CheckpointIdError::NotUuidV7);
        }
        Ok(Self(uuid))
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

#[cfg(test)]
mod tests;
