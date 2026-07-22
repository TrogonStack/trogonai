use serde_json::json;

use crate::ard_identifier::ArdIdentifier;
use crate::catalog_entry::CatalogEntry;
use crate::catalog_entry_wire::{CatalogEntryWire, CatalogEntryWireError};
use crate::metadata::MetadataError;
use crate::representative_queries::RepresentativeQueriesError;
use crate::url_or_data::UrlOrDataError;

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
        capabilities: None,
        version: None,
        updated_at: None,
        metadata: None,
        trust_manifest: None,
    }
}

#[test]
fn converts_valid_wire_to_domain() {
    let entry: CatalogEntry = sample_entry_wire().try_into().unwrap();
    assert_eq!(
        entry.identifier(),
        &ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap()
    );
    assert_eq!(entry.display_name(), "Assistant");
    assert_eq!(entry.media_type().as_str(), "application/a2a-agent-card+json");
    assert!(entry.delivery().url().is_some());
}

#[test]
fn rejects_missing_required_fields() {
    let mut wire = sample_entry_wire();
    wire.identifier = "   ".to_owned();
    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.clone().try_into()),
        Err(CatalogEntryWireError::MissingIdentifier)
    );

    wire = sample_entry_wire();
    wire.display_name = String::new();
    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.try_into()),
        Err(CatalogEntryWireError::MissingDisplayName)
    );

    let mut wire = sample_entry_wire();
    wire.media_type = " ".to_owned();
    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.try_into()),
        Err(CatalogEntryWireError::MissingMediaType)
    );
}

#[test]
fn rejects_invalid_delivery_mode() {
    let mut wire = sample_entry_wire();
    wire.url = Some("https://example.com/card.json".to_owned());
    wire.data = Some(json!({"name": "assistant"}));
    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.try_into()),
        Err(CatalogEntryWireError::Delivery(UrlOrDataError::BothUrlAndData))
    );

    wire = sample_entry_wire();
    wire.url = None;
    wire.data = None;
    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.try_into()),
        Err(CatalogEntryWireError::Delivery(UrlOrDataError::NeitherUrlNorData))
    );
}

#[test]
fn enforces_representative_queries_when_present() {
    let mut wire = sample_entry_wire();
    wire.representative_queries = Some(vec!["only-one".to_owned()]);
    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.try_into()),
        Err(CatalogEntryWireError::RepresentativeQueries(
            RepresentativeQueriesError::TooFew { count: 1 }
        ))
    );
}

#[test]
fn rejects_nested_metadata_values() {
    let mut wire = sample_entry_wire();
    wire.metadata = Some(json!({
        "nested": { "not": "allowed" }
    }));

    assert_eq!(
        Into::<Result<CatalogEntry, CatalogEntryWireError>>::into(wire.try_into()),
        Err(CatalogEntryWireError::Metadata(MetadataError::InvalidValue {
            key: "nested".to_owned()
        }))
    );
}

#[test]
fn round_trips_domain_to_wire() {
    let entry: CatalogEntry = sample_entry_wire().try_into().unwrap();
    let wire = entry.into_wire();
    assert_eq!(wire.identifier, "urn:air:example.com:agent:assistant");
    assert_eq!(wire.display_name, "Assistant");
    assert_eq!(wire.media_type, "application/a2a-agent-card+json");
    assert!(wire.url.is_some());
    assert_eq!(wire.representative_queries.as_ref().map(Vec::len), Some(2));
}

#[test]
fn round_trips_wire_json() {
    let wire = sample_entry_wire();
    let json = serde_json::to_string(&wire).unwrap();
    let parsed: CatalogEntryWire = serde_json::from_str(&json).unwrap();
    let domain: CatalogEntry = parsed.try_into().unwrap();
    let round_trip = domain.into_wire();
    let round_trip_json = serde_json::to_string(&round_trip).unwrap();
    assert_eq!(json, round_trip_json);
}

#[test]
fn into_domain_returns_entry_with_correct_display_name() {
    let wire = sample_entry_wire();
    let entry = wire.into_domain().unwrap();
    assert_eq!(entry.display_name(), "Assistant");
}

#[test]
fn into_domain_exposes_identifier() {
    let wire = sample_entry_wire();
    let entry = wire.into_domain().unwrap();
    assert_eq!(entry.identifier().as_str(), "urn:air:example.com:agent:assistant");
}

#[test]
fn into_domain_exposes_media_type() {
    let wire = sample_entry_wire();
    let entry = wire.into_domain().unwrap();
    assert_eq!(entry.media_type().as_str(), "application/a2a-agent-card+json");
}

#[test]
fn into_domain_returns_err_for_blank_identifier() {
    let mut wire = sample_entry_wire();
    wire.identifier = "  ".to_owned();
    assert_eq!(wire.into_domain(), Err(CatalogEntryWireError::MissingIdentifier));
}
