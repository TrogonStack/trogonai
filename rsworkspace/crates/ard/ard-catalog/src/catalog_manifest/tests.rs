use serde_json::json;

use crate::catalog_entry_wire::CatalogEntryWire;
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
        description: Some("Helpful agent".to_owned()),
        representative_queries: None,
        tags: None,
        capabilities: None,
        version: None,
        updated_at: None,
        metadata: None,
        trust_manifest: None,
    }
}

fn valid_manifest_json() -> serde_json::Value {
    json!({
        "specVersion": "1.0",
        "entries": [{
            "identifier": "urn:air:example.com:agent:assistant",
            "displayName": "Assistant",
            "type": "application/a2a-agent-card+json",
            "url": "https://example.com/card.json"
        }]
    })
}

#[test]
fn exposes_entries_immutably() {
    let manifest: crate::catalog_manifest::CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![CatalogEntryWire {
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
        }],
    }
    .try_into()
    .unwrap();
    assert_eq!(manifest.entries().len(), 1);
    assert_eq!(manifest.spec_version(), SPEC_VERSION);
}

#[test]
fn from_json_value_ok_returns_manifest_with_entries() {
    let manifest = CatalogManifest::from_json_value(valid_manifest_json()).unwrap();
    assert_eq!(manifest.entries().len(), 1);
    assert_eq!(manifest.entries()[0].display_name(), "Assistant");
}

#[test]
fn from_json_value_err_on_schema_invalid_value() {
    let bad = json!({
        "specVersion": "1.0",
        "entries": [{
            "identifier": "not-a-valid-urn",
            "displayName": "Bad",
            "type": "application/a2a-agent-card+json",
            "url": "https://example.com/x.json"
        }]
    });
    let result = CatalogManifest::from_json_value(bad);
    assert!(
        matches!(
            result,
            Err(crate::catalog_manifest::CatalogManifestJsonError::Schema(_))
        ),
        "expected Schema error"
    );
}

#[test]
fn from_json_value_err_on_wrong_spec_version() {
    let bad = json!({
        "specVersion": "2.0",
        "entries": [{
            "identifier": "urn:air:example.com:agent:assistant",
            "displayName": "Assistant",
            "type": "application/a2a-agent-card+json",
            "url": "https://example.com/card.json"
        }]
    });
    let result = CatalogManifest::from_json_value(bad);
    assert!(result.is_err(), "wrong spec version must be rejected");
}

#[test]
fn try_from_wire_domain_error_for_wrong_spec_version() {
    let wire = CatalogManifestWire {
        spec_version: "2.0".to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    };
    let result = CatalogManifest::try_from_wire(wire);
    assert_eq!(result, Err(CatalogManifestWireError::InvalidSpecVersion));
}

#[test]
fn into_json_value_round_trips() {
    let manifest = CatalogManifest::from_json_value(valid_manifest_json()).unwrap();
    let value = manifest.into_json_value().unwrap();
    assert_eq!(value["specVersion"], "1.0");
    assert!(value["entries"].is_array());
    assert_eq!(value["entries"][0]["identifier"], "urn:air:example.com:agent:assistant");
}

#[test]
fn spec_version_returns_constant() {
    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    }
    .try_into()
    .unwrap();
    assert_eq!(manifest.spec_version(), "1.0");
}

#[test]
fn into_entries_consumes_manifest() {
    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    }
    .try_into()
    .unwrap();
    let entries = manifest.into_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].display_name(), "Assistant");
}

#[test]
fn host_returns_some_after_manifest_with_valid_host() {
    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: Some(CatalogHostWire(json!({
            "displayName": "Example Registry"
        }))),
        entries: vec![sample_entry_wire()],
    }
    .try_into()
    .unwrap();
    assert!(manifest.host().is_some());
    assert_eq!(manifest.host().unwrap()["displayName"], "Example Registry");
}

#[test]
fn host_returns_none_when_no_host_set() {
    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![sample_entry_wire()],
    }
    .try_into()
    .unwrap();
    assert!(manifest.host().is_none());
}

#[test]
fn try_from_wire_fails_for_invalid_spec_version() {
    let wire = CatalogManifestWire {
        spec_version: "99.0".to_owned(),
        host: None,
        entries: vec![],
    };
    let result = CatalogManifest::try_from_wire(wire);
    assert_eq!(result, Err(CatalogManifestWireError::InvalidSpecVersion));
}
