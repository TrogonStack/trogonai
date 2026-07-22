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

#[test]
fn list_returns_all_stored_entries() {
    let mut store = MemoryCatalogStore::new();
    assert!(store.list().is_empty(), "new store should be empty");

    store.put(entry()).unwrap();
    let list = store.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].display_name(), "Assistant");
}

#[test]
fn list_does_not_include_deleted_entries() {
    let e = entry();
    let identifier = ArdIdentifier::new(e.identifier().as_str()).unwrap();
    let mut store = MemoryCatalogStore::new();
    store.put(e).unwrap();
    store.delete(&identifier).unwrap();
    assert!(store.list().is_empty());
}

#[test]
fn events_records_upsert_then_delete_in_order() {
    let e = entry();
    let identifier = ArdIdentifier::new(e.identifier().as_str()).unwrap();
    let mut store = MemoryCatalogStore::new();

    store.put(e).unwrap();
    store.delete(&identifier).unwrap();

    let events = store.events();
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], crate::catalog_event::CatalogEvent::Upserted { .. }));
    assert!(matches!(events[1], crate::catalog_event::CatalogEvent::Deleted { .. }));
}

#[test]
fn events_empty_on_new_store() {
    let store = MemoryCatalogStore::new();
    assert!(store.events().is_empty());
}

#[test]
fn put_returns_upserted_event_with_correct_identifier() {
    let mut store = MemoryCatalogStore::new();
    let event = store.put(entry()).unwrap();
    assert_eq!(event.identifier().as_str(), "urn:air:example.com:agent:assistant");
}
