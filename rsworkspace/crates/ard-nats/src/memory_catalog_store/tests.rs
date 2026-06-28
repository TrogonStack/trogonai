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
