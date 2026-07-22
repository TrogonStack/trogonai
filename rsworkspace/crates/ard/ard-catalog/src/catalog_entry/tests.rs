use serde_json::json;

use crate::catalog_entry::CatalogEntry;
use crate::catalog_entry_wire::{CatalogEntryWire, CatalogEntryWireError};

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
fn rejects_invalid_updated_at() {
    let mut wire = sample_entry_wire();
    wire.updated_at = Some("not-a-timestamp".to_owned());
    assert_eq!(
        CatalogEntry::try_from_wire(wire),
        Err(CatalogEntryWireError::InvalidUpdatedAt)
    );

    let mut wire = sample_entry_wire();
    wire.updated_at = Some("2025-01-15".to_owned());
    assert_eq!(
        CatalogEntry::try_from_wire(wire),
        Err(CatalogEntryWireError::InvalidUpdatedAt)
    );
}

#[test]
fn accepts_rfc3339_updated_at() {
    let mut wire = sample_entry_wire();
    wire.updated_at = Some("2025-01-15T08:30:00Z".to_owned());
    let entry = CatalogEntry::try_from_wire(wire).unwrap();
    assert_eq!(entry.updated_at(), Some("2025-01-15T08:30:00Z"));
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

#[test]
fn exposes_optional_entry_accessors() {
    let wire = CatalogEntryWire {
        identifier: "urn:air:example.com:agent:full".to_owned(),
        display_name: "Full Entry".to_owned(),
        media_type: "application/a2a-agent-card+json".to_owned(),
        url: Some("https://example.com/full.json".to_owned()),
        data: None,
        description: None,
        representative_queries: None,
        tags: None,
        capabilities: None,
        version: Some("2.3.1".to_owned()),
        updated_at: Some("2025-01-15T00:00:00Z".to_owned()),
        metadata: Some(json!({"env": "prod"})),
        trust_manifest: Some(json!({
            "identity": "did:web:example.com",
            "identityType": "did"
        })),
    };

    let entry = CatalogEntry::try_from_wire(wire).unwrap();

    assert_eq!(entry.version(), Some("2.3.1"));
    assert_eq!(entry.updated_at(), Some("2025-01-15T00:00:00Z"));
    assert_eq!(entry.metadata().map(|v| &v["env"]), Some(&json!("prod")));
    assert!(entry.trust_manifest().is_some());
}

#[test]
fn accessors_return_none_when_fields_absent() {
    let entry = CatalogEntry::try_from_wire(sample_entry_wire()).unwrap();
    assert_eq!(entry.version(), None);
    assert_eq!(entry.updated_at(), None);
    assert_eq!(entry.metadata(), None);
    assert_eq!(entry.trust_manifest(), None);
}
