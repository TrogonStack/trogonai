use ard_catalog::{ArdIdentifier, ArdStorageKey, CatalogEntry};

use crate::catalog_event::CatalogEvent;

#[derive(Debug, thiserror::Error)]
pub enum CatalogStoreError {
    #[error("catalog entry not found: {0}")]
    NotFound(ArdIdentifier),
    #[error(transparent)]
    CatalogEntry(#[from] ard_catalog::CatalogEntryWireError),
    #[error(transparent)]
    Identifier(#[from] ard_catalog::ArdIdentifierError),
}

pub trait CatalogStore {
    fn put(&mut self, entry: CatalogEntry) -> Result<CatalogEvent, CatalogStoreError>;
    fn get(&self, identifier: &ArdIdentifier) -> Result<CatalogEntry, CatalogStoreError>;
    fn delete(&mut self, identifier: &ArdIdentifier) -> Result<CatalogEvent, CatalogStoreError>;
    fn list(&self) -> Vec<CatalogEntry>;
    fn events(&self) -> &[CatalogEvent];
}

pub(crate) fn key_for(identifier: &ArdIdentifier) -> ArdStorageKey {
    ArdStorageKey::from_identifier(identifier)
}
