use serde_json::json;

use crate::catalog_manifest::{CatalogManifest, CatalogManifestJsonError};

use super::*;

fn valid_manifest() -> serde_json::Value {
    json!({
        "specVersion": "1.0",
        "entries": [{
            "identifier": "urn:air:example.com:agent:assistant",
            "displayName": "Assistant",
            "type": "application/a2a-agent-card+json",
            "url": "https://example.com/card.json",
            "representativeQueries": ["help me", "answer a question"],
            "trustManifest": {
                "identity": "spiffe://example.com/agent/assistant",
                "identityType": "spiffe"
            },
            "metadata": {
                "tier": "internal",
                "enabled": true
            }
        }]
    })
}

#[test]
fn accepts_valid_manifest_json() {
    validate_ai_catalog_value(&valid_manifest()).expect("manifest should validate");
}

#[test]
fn parses_valid_manifest_through_schema_and_domain() {
    let manifest = CatalogManifest::from_json_value(valid_manifest()).unwrap();
    assert_eq!(manifest.entries().len(), 1);
    assert!(manifest.entries()[0].trust_manifest().is_some());
}

#[test]
fn round_trips_primary_manifest_json_path() {
    let value = valid_manifest();
    let manifest = CatalogManifest::from_json_value(value.clone()).unwrap();
    let round_trip = manifest.into_json_value().unwrap();
    assert_eq!(round_trip, value);
}

#[test]
fn rejects_schema_valid_but_domain_invalid_trust_manifest() {
    let mut manifest = valid_manifest();
    manifest["entries"][0]["trustManifest"]["identity"] = json!("");

    assert!(matches!(
        CatalogManifest::from_json_value(manifest),
        Err(CatalogManifestJsonError::Domain(_))
    ));
}

#[test]
fn rejects_deprecated_collections_field() {
    let mut manifest = valid_manifest();
    manifest["collections"] = json!([]);
    let err = validate_ai_catalog_value(&manifest).unwrap_err();
    assert!(
        err.violations()
            .iter()
            .any(|v| v.message().contains("collections") || v.instance_path().contains("collections"))
    );
}

#[test]
fn rejects_invalid_urn_ai_identifier() {
    let mut manifest = valid_manifest();
    manifest["entries"][0]["identifier"] = json!("urn:ai:example.com:agent:assistant");
    let err = validate_ai_catalog_value(&manifest).unwrap_err();
    assert!(
        err.violations()
            .iter()
            .any(|v| v.message().contains("identifier") || v.instance_path().contains("identifier"))
    );
}

#[test]
fn rejects_both_url_and_data() {
    let mut manifest = valid_manifest();
    manifest["entries"][0]["data"] = json!({ "name": "assistant" });
    let err = validate_ai_catalog_value(&manifest).unwrap_err();
    assert!(!err.violations().is_empty());
}

#[test]
fn rejects_unknown_top_level_entry_field() {
    let mut manifest = valid_manifest();
    manifest["entries"][0]["somethingUnknown"] = json!("x");
    assert!(validate_ai_catalog_value(&manifest).is_err());
}
