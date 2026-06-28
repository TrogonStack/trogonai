use std::collections::BTreeMap;

use ard_catalog::{ArdIdentifier, CatalogEntry};

use crate::catalog_event::CatalogEvent;
use crate::store::{CatalogStore, CatalogStoreError, key_for};

#[derive(Debug, Clone, Default)]
pub struct MemoryCatalogStore {
    entries: BTreeMap<String, CatalogEntry>,
    events: Vec<CatalogEvent>,
}

impl MemoryCatalogStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CatalogStore for MemoryCatalogStore {
    fn put(&mut self, entry: CatalogEntry) -> Result<CatalogEvent, CatalogStoreError> {
        let key = key_for(entry.identifier()).to_string();
        self.entries.insert(key, entry.clone());
        let event = CatalogEvent::upserted(&entry);
        self.events.push(event.clone());
        Ok(event)
    }

    fn get(&self, identifier: &ArdIdentifier) -> Result<CatalogEntry, CatalogStoreError> {
        let key = key_for(identifier).to_string();
        self.entries
            .get(&key)
            .cloned()
            .ok_or_else(|| CatalogStoreError::NotFound(identifier.to_string()))
    }

    fn delete(&mut self, identifier: &ArdIdentifier) -> Result<CatalogEvent, CatalogStoreError> {
        let key = key_for(identifier).to_string();
        self.entries
            .remove(&key)
            .ok_or_else(|| CatalogStoreError::NotFound(identifier.to_string()))?;
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
mod tests {
    use ard_catalog::{ArdIdentifier, CatalogEntryWire};

    use super::MemoryCatalogStore;
    use crate::store::CatalogStore;

    fn entry() -> ard_catalog::CatalogEntry {
        CatalogEntryWire {
            identifier: "urn:air:example.com:agent:assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some("https://example.com/card.json".to_owned()),
            data: None,
            description: Some("Helpful assistant".to_owned()),
            representative_queries: Some(vec!["help me".to_owned(), "answer questions".to_owned()]),
            tags: Some(vec!["demo".to_owned()]),
            capabilities: Some(vec!["chat".to_owned()]),
            version: None,
            updated_at: None,
            metadata: None,
            trust_manifest: None,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn put_get_delete_records_events() {
        let entry = entry();
        let identifier = ArdIdentifier::new(entry.identifier().as_str()).unwrap();
        let mut store = MemoryCatalogStore::new();

        store.put(entry).unwrap();
        assert_eq!(store.get(&identifier).unwrap().display_name(), "Assistant");

        store.delete(&identifier).unwrap();
        assert!(store.get(&identifier).is_err());
        assert_eq!(store.events().len(), 2);
    }
}
