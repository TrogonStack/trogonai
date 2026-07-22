use ard_catalog::{ArdIdentifier, ArdStorageKey, CatalogEntry, CatalogEntryWire};
use serde::{Deserialize, Serialize};

use crate::store::CatalogStoreError;

/// Domain event — validated types only, no serde.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogEvent {
    Upserted { entry: Box<CatalogEntry> },
    Deleted { identifier: ArdIdentifier },
    Validated { identifier: ArdIdentifier },
    Indexed { identifier: ArdIdentifier },
}

impl CatalogEvent {
    pub fn upserted(entry: &CatalogEntry) -> Self {
        Self::Upserted {
            entry: Box::new(entry.clone()),
        }
    }

    pub fn deleted(identifier: &ArdIdentifier) -> Self {
        Self::Deleted {
            identifier: identifier.clone(),
        }
    }

    pub fn identifier(&self) -> &ArdIdentifier {
        match self {
            Self::Upserted { entry } => entry.identifier(),
            Self::Deleted { identifier } | Self::Validated { identifier } | Self::Indexed { identifier } => identifier,
        }
    }

    pub fn storage_key(&self) -> ArdStorageKey {
        ArdStorageKey::from_identifier(self.identifier())
    }

    pub fn into_wire(self) -> CatalogEventWire {
        match self {
            Self::Upserted { entry } => CatalogEventWire::Upserted {
                storage_key: ArdStorageKey::from_identifier(entry.identifier()).to_string(),
                entry: Box::new((*entry).into_wire()),
            },
            Self::Deleted { identifier } => CatalogEventWire::Deleted {
                storage_key: ArdStorageKey::from_identifier(&identifier).to_string(),
                identifier: identifier.to_string(),
            },
            Self::Validated { identifier } => CatalogEventWire::Validated {
                storage_key: ArdStorageKey::from_identifier(&identifier).to_string(),
                identifier: identifier.to_string(),
            },
            Self::Indexed { identifier } => CatalogEventWire::Indexed {
                storage_key: ArdStorageKey::from_identifier(&identifier).to_string(),
                identifier: identifier.to_string(),
            },
        }
    }
}

impl TryFrom<CatalogEventWire> for CatalogEvent {
    type Error = CatalogStoreError;

    fn try_from(wire: CatalogEventWire) -> Result<Self, Self::Error> {
        match wire {
            CatalogEventWire::Upserted { entry, storage_key } => {
                let entry: CatalogEntry = (*entry).try_into()?;
                let expected_key = ArdStorageKey::from_identifier(entry.identifier());
                if expected_key.as_str() != storage_key {
                    return Err(CatalogStoreError::StorageKeyMismatch);
                }
                Ok(Self::Upserted { entry: Box::new(entry) })
            }
            CatalogEventWire::Deleted {
                identifier,
                storage_key,
            } => {
                let id = ArdIdentifier::new(identifier)?;
                let expected_key = ArdStorageKey::from_identifier(&id);
                if expected_key.as_str() != storage_key {
                    return Err(CatalogStoreError::StorageKeyMismatch);
                }
                Ok(Self::Deleted { identifier: id })
            }
            CatalogEventWire::Validated {
                identifier,
                storage_key,
            } => {
                let id = ArdIdentifier::new(identifier)?;
                let expected_key = ArdStorageKey::from_identifier(&id);
                if expected_key.as_str() != storage_key {
                    return Err(CatalogStoreError::StorageKeyMismatch);
                }
                Ok(Self::Validated { identifier: id })
            }
            CatalogEventWire::Indexed {
                identifier,
                storage_key,
            } => {
                let id = ArdIdentifier::new(identifier)?;
                let expected_key = ArdStorageKey::from_identifier(&id);
                if expected_key.as_str() != storage_key {
                    return Err(CatalogStoreError::StorageKeyMismatch);
                }
                Ok(Self::Indexed { identifier: id })
            }
        }
    }
}

/// Wire representation — serde-serializable, preserves on-wire JSON shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "eventType", rename_all = "camelCase")]
pub enum CatalogEventWire {
    Upserted {
        storage_key: String,
        entry: Box<CatalogEntryWire>,
    },
    Deleted {
        identifier: String,
        storage_key: String,
    },
    Validated {
        identifier: String,
        storage_key: String,
    },
    Indexed {
        identifier: String,
        storage_key: String,
    },
}

#[cfg(test)]
mod tests;
