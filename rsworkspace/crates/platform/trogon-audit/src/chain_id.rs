use uuid::Uuid;

use crate::constants::CHAIN_ID_UUID_VERSION;
use trogon_std::uuid::NowV7;

/// One chain of a writer: a UUIDv7 carried in `integrity.chain_id` and the
/// `Trogon-Audit-Chain` header. Every process start begins a new chain under the
/// same writer id, so a chain is named by the writer id and the chain id
/// together (ADR#0086 Decision 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChainId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChainIdError {
    #[error("chain id must be a UUIDv7")]
    NotUuidV7,
}

impl ChainId {
    pub fn generate(generator: &impl NowV7) -> Self {
        Self(generator.now_v7())
    }

    pub fn from_uuid(uuid: Uuid) -> Result<Self, ChainIdError> {
        if uuid.get_version_num() != CHAIN_ID_UUID_VERSION {
            return Err(ChainIdError::NotUuidV7);
        }
        Ok(Self(uuid))
    }

    /// Reads the hyphenated lowercase form the writer stores, so one chain has
    /// one spelling.
    pub fn parse(value: &str) -> Result<Self, ChainIdError> {
        let uuid = Uuid::try_parse(value).map_err(|_| ChainIdError::NotUuidV7)?;
        if uuid.hyphenated().to_string() != value {
            return Err(ChainIdError::NotUuidV7);
        }
        Self::from_uuid(uuid)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

#[cfg(test)]
mod tests;
