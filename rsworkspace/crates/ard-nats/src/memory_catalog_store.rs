use std::collections::BTreeMap;

use ard_catalog::{ArdIdentifier, ArdStorageKey, CatalogEntry};

use crate::catalog_event::CatalogEvent;
use crate::store::{CatalogStore, CatalogStoreError, key_for};

#[derive(Debug, Clone, Default)]
pub struct MemoryCatalogStore {
    entries: BTreeMap<ArdStorageKey, CatalogEntry>,
    events: Vec<CatalogEvent>,
}

impl MemoryCatalogStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CatalogStore for MemoryCatalogStore {
    fn put(&mut self, entry: CatalogEntry) -> Result<CatalogEvent, CatalogStoreError> {
        let key = key_for(entry.identifier());
        self.entries.insert(key, entry.clone());
        let event = CatalogEvent::upserted(&entry);
        self.events.push(event.clone());
        Ok(event)
    }

    fn get(&self, identifier: &ArdIdentifier) -> Result<CatalogEntry, CatalogStoreError> {
        let key = key_for(identifier);
        self.entries
            .get(&key)
            .cloned()
            .ok_or_else(|| CatalogStoreError::NotFound(identifier.clone()))
    }

    fn delete(&mut self, identifier: &ArdIdentifier) -> Result<CatalogEvent, CatalogStoreError> {
        let key = key_for(identifier);
        self.entries
            .remove(&key)
            .ok_or_else(|| CatalogStoreError::NotFound(identifier.clone()))?;
        let event = CatalogEvent::deleted(identifier);
        self.events.push(event.clone());
        Ok(event)
    }

    fn list(&self) -> Vec<CatalogEntry> {
        self.entries.values().cloned().collect()
    }

    fn events(&self) -> &[CatalogEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests;
