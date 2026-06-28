use serde_json::json;

use super::*;

#[test]
fn accepts_display_name() {
    let host = CatalogHost::new(json!({
        "displayName": "Example",
        "trustManifest": {
            "identity": "did:web:example.com",
            "identityType": "did"
        }
    }))
    .unwrap();

    assert_eq!(host.as_value()["displayName"], "Example");
}

#[test]
fn rejects_missing_display_name() {
    assert_eq!(CatalogHost::new(json!({})), Err(CatalogHostError::MissingDisplayName));
}

#[test]
fn rejects_invalid_trust_manifest() {
    assert_eq!(
        CatalogHost::new(json!({
            "displayName": "Example",
            "trustManifest": {}
        })),
        Err(CatalogHostError::TrustManifest(TrustManifestError::MissingIdentity))
    );
}
