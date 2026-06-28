use serde_json::json;

use super::*;

#[test]
fn accepts_identity_metadata() {
    let trust = TrustManifest::new(json!({
        "identity": "spiffe://example.com/agent/assistant",
        "identityType": "spiffe"
    }))
    .unwrap();

    assert_eq!(trust.as_value()["identity"], "spiffe://example.com/agent/assistant");
}

#[test]
fn rejects_missing_identity() {
    assert_eq!(TrustManifest::new(json!({})), Err(TrustManifestError::MissingIdentity));
}

#[test]
fn rejects_unknown_identity_type() {
    assert_eq!(
        TrustManifest::new(json!({
            "identity": "did:web:example.com",
            "identityType": "jwt"
        })),
        Err(TrustManifestError::InvalidIdentityType)
    );
}

#[test]
fn rejects_unknown_field() {
    assert!(matches!(
        TrustManifest::new(json!({
            "identity": "did:web:example.com",
            "unknownExtra": "rejected"
        })),
        Err(TrustManifestError::UnknownField(_))
    ));
}

#[test]
fn accepts_all_allowed_keys_and_round_trips() {
    let trust = TrustManifest::new(json!({
        "identity": "did:web:example.com",
        "identityType": "did",
        "trustSchema": "https://schema.example/v1",
        "attestations": [],
        "provenance": "https://prov.example",
        "signature": "abc123"
    }))
    .unwrap();
    assert_eq!(trust.as_value()["identity"], "did:web:example.com");
    assert_eq!(trust.as_value()["identityType"], "did");
}
