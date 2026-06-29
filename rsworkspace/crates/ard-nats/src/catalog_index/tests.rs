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

#[test]
fn manifest_returns_all_entries_after_replay() {
    let mut store = MemoryCatalogStore::new();
    store.put(entry("urn:air:example.com:agent:alpha", "Alpha")).unwrap();
    store.put(entry("urn:air:example.com:agent:beta", "Beta")).unwrap();

    let index = CatalogIndex::replay(store.events());
    let manifest = index.manifest().unwrap();
    assert_eq!(manifest.entries().len(), 2);
}

#[test]
fn search_empty_query_returns_no_results() {
    let mut store = MemoryCatalogStore::new();
    store
        .put(entry("urn:air:example.com:agent:assistant", "Assistant"))
        .unwrap();
    let index = CatalogIndex::replay(store.events());

    let results = index.search("", "https://registry.example.com");
    assert!(results.is_empty(), "empty query must return nothing");
}

#[test]
fn search_whitespace_only_query_returns_no_results() {
    let mut store = MemoryCatalogStore::new();
    store
        .put(entry("urn:air:example.com:agent:assistant", "Assistant"))
        .unwrap();
    let index = CatalogIndex::replay(store.events());

    let results = index.search("   ", "https://registry.example.com");
    assert!(results.is_empty());
}

#[test]
fn search_ranks_by_score_descending() {
    let mut store = MemoryCatalogStore::new();
    store
        .put(entry("urn:air:example.com:agent:alpha", "Coding Alpha"))
        .unwrap();
    store
        .put(entry("urn:air:example.com:agent:beta", "Beta Helper"))
        .unwrap();

    let index = CatalogIndex::replay(store.events());
    let results = index.search("coding", "https://registry.example.com");

    assert!(!results.is_empty());
    assert_eq!(results[0].entry.identifier, "urn:air:example.com:agent:alpha");
}

#[test]
fn search_tie_break_uses_identifier_order() {
    let mut store = MemoryCatalogStore::new();
    store
        .put(entry("urn:air:example.com:agent:beta", "Coding Bot"))
        .unwrap();
    store
        .put(entry("urn:air:example.com:agent:alpha", "Coding Bot"))
        .unwrap();

    let index = CatalogIndex::replay(store.events());
    let results = index.search("coding", "https://registry.example.com");

    assert_eq!(results.len(), 2);
    assert!(
        results[0].entry.identifier <= results[1].entry.identifier,
        "tie-break should be alphabetical by identifier"
    );
}

#[test]
fn new_index_has_empty_manifest() {
    let index = CatalogIndex::new();
    let manifest = index.manifest().unwrap();
    assert!(manifest.entries().is_empty());
}

#[test]
fn apply_upserted_event_adds_entry() {
    let mut store = MemoryCatalogStore::new();
    let event = store
        .put(entry("urn:air:example.com:agent:assistant", "Assistant"))
        .unwrap();

    let mut index = CatalogIndex::new();
    index.apply(event);
    assert_eq!(index.manifest().unwrap().entries().len(), 1);
}
