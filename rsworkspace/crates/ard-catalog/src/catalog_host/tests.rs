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

#[test]
fn rejects_non_string_display_name() {
    assert_eq!(
        CatalogHost::new(json!({"displayName": 42})),
        Err(CatalogHostError::InvalidDisplayName)
    );
}

#[test]
fn rejects_non_object() {
    assert_eq!(CatalogHost::new(json!("string")), Err(CatalogHostError::NotObject));
    assert_eq!(CatalogHost::new(json!(null)), Err(CatalogHostError::NotObject));
    assert_eq!(CatalogHost::new(json!([1, 2, 3])), Err(CatalogHostError::NotObject));
}

#[test]
fn rejects_unknown_field() {
    assert!(matches!(
        CatalogHost::new(json!({
            "displayName": "Acme",
            "extra": "rejected"
        })),
        Err(CatalogHostError::UnknownField(_))
    ));
}

#[test]
fn accepts_only_allowed_keys_and_round_trips() {
    let host = CatalogHost::new(json!({
        "displayName": "Acme",
        "trustManifest": {
            "identity": "did:web:acme.example",
            "identityType": "did"
        }
    }))
    .unwrap();
    let value = host.into_value();
    assert_eq!(value["displayName"], "Acme");
    assert_eq!(value["trustManifest"]["identity"], "did:web:acme.example");
}
