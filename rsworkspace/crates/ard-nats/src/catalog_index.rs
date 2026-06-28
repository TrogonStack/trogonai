use std::collections::BTreeMap;

use ard_catalog::{
    ArdStorageKey, CatalogEntry, CatalogEntryWire, CatalogManifest, CatalogManifestWire, CatalogManifestWireError,
    SearchResultWire, SPEC_VERSION,
};

use crate::catalog_event::CatalogEvent;

#[derive(Debug, Clone, Default)]
pub struct CatalogIndex {
    entries: BTreeMap<ArdStorageKey, CatalogEntry>,
}

impl CatalogIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replay(events: &[CatalogEvent]) -> Self {
        let mut index = Self::new();
        for event in events {
            index.apply(event.clone());
        }
        index
    }

    pub fn apply(&mut self, event: CatalogEvent) {
        match event {
            CatalogEvent::Upserted { entry } => {
                let key = ArdStorageKey::from_identifier(entry.identifier());
                self.entries.insert(key, *entry);
            }
            CatalogEvent::Deleted { identifier } => {
                self.entries.remove(&ArdStorageKey::from_identifier(&identifier));
            }
            CatalogEvent::Validated { .. } | CatalogEvent::Indexed { .. } => {}
        }
    }

    pub fn manifest(&self) -> Result<CatalogManifest, CatalogManifestWireError> {
        CatalogManifestWire {
            spec_version: SPEC_VERSION.to_owned(),
            host: None,
            entries: self.entries.values().cloned().map(CatalogEntryWire::from).collect(),
        }
        .try_into()
    }

    pub fn search(&self, query: &str, source: &str) -> Vec<SearchResultWire> {
        let mut results = self
            .entries
            .values()
            .filter_map(|entry| {
                let score = lexical_score(query, entry);
                (score > 0).then(|| SearchResultWire {
                    entry: entry.clone().into_wire(),
                    score,
                    source: source.to_owned(),
                })
            })
            .collect::<Vec<_>>();

        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.entry.identifier.cmp(&right.entry.identifier))
        });
        results
    }
}

fn lexical_score(query: &str, entry: &CatalogEntry) -> u8 {
    let query = query.trim();
    if query.is_empty() {
        return 0;
    }

    let normalized_query = query.to_lowercase();
    let mut score = 0u8;
    for term in normalized_query.split_whitespace() {
        let display_name = entry.display_name().to_lowercase();
        let description = entry.description().unwrap_or_default().to_lowercase();
        let tags = entry
            .tags()
            .map(|values| values.join(" ").to_lowercase())
            .unwrap_or_default();
        let capabilities = entry
            .capabilities()
            .map(|values| values.join(" ").to_lowercase())
            .unwrap_or_default();

        if display_name.contains(term) {
            score = score.saturating_add(40);
        }
        if description.contains(term) {
            score = score.saturating_add(25);
        }
        if tags.contains(term) {
            score = score.saturating_add(20);
        }
        if capabilities.contains(term) {
            score = score.saturating_add(15);
        }
    }
    score.min(100)
}

#[cfg(test)]
mod tests {
    use ard_catalog::{ArdIdentifier, CatalogEntryWire};

    use super::CatalogIndex;
    use crate::memory_catalog_store::MemoryCatalogStore;
    use crate::store::CatalogStore;

    fn entry(identifier: &str, name: &str) -> ard_catalog::CatalogEntry {
        CatalogEntryWire {
            identifier: identifier.to_owned(),
            display_name: name.to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some(format!("https://example.com/{name}.json")),
            data: None,
            description: Some("Helpful coding assistant".to_owned()),
            representative_queries: Some(vec!["write code".to_owned(), "debug rust".to_owned()]),
            tags: Some(vec!["coding".to_owned()]),
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
    fn replay_reconstructs_index_from_store_events() {
        let mut store = MemoryCatalogStore::new();
        store
            .put(entry("urn:air:example.com:agent:assistant", "Assistant"))
            .unwrap();

        let index = CatalogIndex::replay(store.events());
        let results = index.search("assistant", "https://registry.example.com");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.identifier, "urn:air:example.com:agent:assistant");
        assert!(!serde_json::to_string(&results[0]).unwrap().contains("ard.catalog"));
    }

    #[test]
    fn delete_event_removes_entry_from_index() {
        let mut store = MemoryCatalogStore::new();
        store
            .put(entry("urn:air:example.com:agent:assistant", "Assistant"))
            .unwrap();
        store
            .delete(&ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap())
            .unwrap();

        let index = CatalogIndex::replay(store.events());

        assert!(index.search("assistant", "https://registry.example.com").is_empty());
        assert!(index.manifest().unwrap().entries().is_empty());
    }
}
