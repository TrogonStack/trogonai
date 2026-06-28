use crate::catalog_entry::CatalogEntry;
use crate::catalog_entry_wire::CatalogEntryWire;

fn sample_entry_wire() -> CatalogEntryWire {
    CatalogEntryWire {
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
        display_name: "Assistant".to_owned(),
        media_type: "application/a2a-agent-card+json".to_owned(),
        url: Some("https://example.com/card.json".to_owned()),
        data: None,
        description: Some("Helpful agent".to_owned()),
        representative_queries: Some(vec!["hello".to_owned(), "help me".to_owned()]),
        tags: Some(vec!["demo".to_owned()]),
        capabilities: Some(vec!["chat".to_owned()]),
        version: None,
        updated_at: None,
        metadata: None,
        trust_manifest: None,
    }
}

#[test]
fn exposes_validated_entry_fields() {
    let entry = CatalogEntry::try_from_wire(sample_entry_wire()).unwrap();

    assert_eq!(entry.identifier().as_str(), "urn:air:example.com:agent:assistant");
    assert_eq!(entry.display_name(), "Assistant");
    assert_eq!(entry.media_type().as_str(), "application/a2a-agent-card+json");
    assert_eq!(entry.description(), Some("Helpful agent"));
    assert_eq!(entry.tags().map(|tags| tags.len()), Some(1));
    assert_eq!(entry.capabilities().map(|capabilities| capabilities.len()), Some(1));
    assert!(entry.delivery().url().is_some());
    assert_eq!(
        entry.representative_queries().map(|queries| queries.as_slice().len()),
        Some(2)
    );
}
