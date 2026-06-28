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
