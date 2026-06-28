use ard_catalog::{ArdIdentifier, CatalogEntryWire};

use super::{CatalogEvent, CatalogEventWire};

use crate::store::CatalogStoreError;

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

fn identifier() -> ArdIdentifier {
    ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap()
}

#[test]
fn event_round_trips_as_json() {
    let event = CatalogEvent::upserted(&entry());
    let wire = event.into_wire();
    let json = serde_json::to_vec(&wire).unwrap();
    let decoded: CatalogEventWire = serde_json::from_slice(&json).unwrap();
    assert_eq!(decoded, wire);
}

#[test]
fn wire_converts_back_to_domain() {
    let event = CatalogEvent::upserted(&entry());
    let wire = event.into_wire();
    let json = serde_json::to_vec(&wire).unwrap();
    let decoded: CatalogEventWire = serde_json::from_slice(&json).unwrap();
    assert!(CatalogEvent::try_from(decoded).is_ok());
}

#[test]
fn deleted_identifier_and_storage_key() {
    let id = identifier();
    let event = CatalogEvent::deleted(&id);
    assert_eq!(event.identifier().as_str(), id.as_str());
    let key = event.storage_key();
    assert!(!key.to_string().is_empty());
}

#[test]
fn deleted_into_wire_round_trips() {
    let id = identifier();
    let event = CatalogEvent::deleted(&id);
    let wire = event.into_wire();
    let json = serde_json::to_vec(&wire).unwrap();
    let decoded: CatalogEventWire = serde_json::from_slice(&json).unwrap();
    let domain = CatalogEvent::try_from(decoded).unwrap();
    assert!(matches!(domain, CatalogEvent::Deleted { .. }));
}

#[test]
fn validated_identifier_and_storage_key() {
    let id = identifier();
    let event = CatalogEvent::Validated { identifier: id.clone() };
    assert_eq!(event.identifier().as_str(), id.as_str());
    let key = event.storage_key();
    assert!(!key.to_string().is_empty());
}

#[test]
fn validated_into_wire_round_trips() {
    let id = identifier();
    let event = CatalogEvent::Validated { identifier: id };
    let wire = event.into_wire();
    let json = serde_json::to_vec(&wire).unwrap();
    let decoded: CatalogEventWire = serde_json::from_slice(&json).unwrap();
    let domain = CatalogEvent::try_from(decoded).unwrap();
    assert!(matches!(domain, CatalogEvent::Validated { .. }));
}

#[test]
fn indexed_into_wire_round_trips() {
    let id = identifier();
    let event = CatalogEvent::Indexed { identifier: id };
    let wire = event.into_wire();
    let json = serde_json::to_vec(&wire).unwrap();
    let decoded: CatalogEventWire = serde_json::from_slice(&json).unwrap();
    let domain = CatalogEvent::try_from(decoded).unwrap();
    assert!(matches!(domain, CatalogEvent::Indexed { .. }));
}

#[test]
fn upserted_identifier_and_storage_key() {
    let e = entry();
    let event = CatalogEvent::upserted(&e);
    assert_eq!(event.identifier().as_str(), e.identifier().as_str());
    let key = event.storage_key();
    assert!(!key.to_string().is_empty());
}

#[test]
fn try_from_wire_deleted_bad_identifier_errors() {
    let wire = CatalogEventWire::Deleted {
        storage_key: "some/key".to_owned(),
        identifier: "not-a-urn".to_owned(),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::Identifier(_))));
}

#[test]
fn try_from_wire_validated_bad_identifier_errors() {
    let wire = CatalogEventWire::Validated {
        storage_key: "some/key".to_owned(),
        identifier: "not-a-urn".to_owned(),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::Identifier(_))));
}

#[test]
fn try_from_wire_indexed_bad_identifier_errors() {
    let wire = CatalogEventWire::Indexed {
        storage_key: "some/key".to_owned(),
        identifier: "not-a-urn".to_owned(),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::Identifier(_))));
}

#[test]
fn try_from_wire_deleted_storage_key_mismatch_errors() {
    let wire = CatalogEventWire::Deleted {
        storage_key: "wrong-key".to_owned(),
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::StorageKeyMismatch)));
}

#[test]
fn try_from_wire_validated_storage_key_mismatch_errors() {
    let wire = CatalogEventWire::Validated {
        storage_key: "wrong-key".to_owned(),
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::StorageKeyMismatch)));
}

#[test]
fn try_from_wire_indexed_storage_key_mismatch_errors() {
    let wire = CatalogEventWire::Indexed {
        storage_key: "wrong-key".to_owned(),
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::StorageKeyMismatch)));
}

#[test]
fn try_from_wire_upserted_storage_key_mismatch_errors() {
    let valid_wire_entry = CatalogEntryWire {
        identifier: "urn:air:example.com:agent:assistant".to_owned(),
        display_name: "Assistant".to_owned(),
        media_type: "application/a2a-agent-card+json".to_owned(),
        url: Some("https://example.com/card.json".to_owned()),
        data: None,
        description: None,
        representative_queries: None,
        tags: None,
        capabilities: None,
        version: None,
        updated_at: None,
        metadata: None,
        trust_manifest: None,
    };
    let wire = CatalogEventWire::Upserted {
        storage_key: "wrong-key".to_owned(),
        entry: Box::new(valid_wire_entry),
    };
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Err(CatalogStoreError::StorageKeyMismatch)));
}

#[test]
fn correctly_derived_storage_key_round_trips_ok() {
    let id = identifier();
    let event = CatalogEvent::deleted(&id);
    let wire = event.into_wire();
    let result = CatalogEvent::try_from(wire);
    assert!(matches!(result, Ok(CatalogEvent::Deleted { .. })));
}
