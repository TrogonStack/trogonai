use serde_json::json;

use crate::catalog_entry_wire::CatalogEntryWire;
use crate::catalog_host::CatalogHostError;
use crate::catalog_host_wire::CatalogHostWire;
use crate::catalog_manifest::CatalogManifest;
use crate::catalog_manifest_wire::{CatalogManifestWire, CatalogManifestWireError, SPEC_VERSION};

fn sample_entry_wire() -> CatalogEntryWire {
    CatalogEntryWire {
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
    }
}

#[test]
fn converts_valid_manifest_wire() {
    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    }
    .try_into()
    .unwrap();
    assert_eq!(manifest.spec_version(), SPEC_VERSION);
    assert_eq!(manifest.entries().len(), 1);
}

#[test]
fn rejects_invalid_spec_version() {
    let manifest = CatalogManifestWire {
        spec_version: "0.9".to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    };
    assert_eq!(
        Into::<Result<CatalogManifest, CatalogManifestWireError>>::into(manifest.try_into()),
        Err(CatalogManifestWireError::InvalidSpecVersion)
    );
}

#[test]
fn rejects_invalid_host_metadata() {
    let manifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: Some(CatalogHostWire(json!({}))),
        entries: vec![sample_entry_wire()],
    };

    assert_eq!(
        Into::<Result<CatalogManifest, CatalogManifestWireError>>::into(manifest.try_into()),
        Err(CatalogManifestWireError::Host(CatalogHostError::MissingDisplayName))
    );
}

#[test]
fn round_trips_manifest_wire_json() {
    let wire = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    };
    let json = serde_json::to_string(&wire).unwrap();
    let parsed: CatalogManifestWire = serde_json::from_str(&json).unwrap();
    let domain = parsed.into_domain().unwrap();
    let round_trip = domain.into_wire();
    let round_trip_json = serde_json::to_string(&round_trip).unwrap();
    assert_eq!(json, round_trip_json);
}

#[test]
fn round_trips_manifest_with_data_entry() {
    let wire = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![CatalogEntryWire {
            identifier: "urn:air:example.com:agent:embedded".to_owned(),
            display_name: "Embedded".to_owned(),
            media_type: "application/vnd.example.custom+proto".to_owned(),
            url: None,
            data: Some(json!({"hello": "world"})),
            description: Some("embedded card".to_owned()),
            representative_queries: Some(vec!["one".to_owned(), "two".to_owned()]),
            tags: Some(vec!["demo".to_owned()]),
            capabilities: Some(vec!["chat".to_owned()]),
            version: None,
            updated_at: None,
            metadata: None,
            trust_manifest: None,
        }],
    };
    let domain: CatalogManifest = wire.clone().try_into().unwrap();
    let round_trip: CatalogManifestWire = domain.into();
    assert_eq!(wire, round_trip);
}

#[test]
fn propagates_entry_errors_with_index() {
    let manifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![sample_entry_wire(), {
            let mut invalid = sample_entry_wire();
            invalid.url = None;
            invalid.data = None;
            invalid
        }],
    };
    assert!(matches!(
        Into::<Result<CatalogManifest, CatalogManifestWireError>>::into(manifest.try_into()),
        Err(CatalogManifestWireError::Entry { index: 1, .. })
    ));
}
