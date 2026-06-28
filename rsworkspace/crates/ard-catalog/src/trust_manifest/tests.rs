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
        "trustSchema": { "identifier": "trust-v1" },
        "attestations": [],
        "provenance": [],
        "signature": "abc123"
    }))
    .unwrap();
    assert_eq!(trust.as_value()["identity"], "did:web:example.com");
    assert_eq!(trust.as_value()["identityType"], "did");
}

#[test]
fn rejects_non_object_trust_schema() {
    assert_eq!(
        TrustManifest::new(json!({"identity": "did:web:example.com", "trustSchema": "v1"})),
        Err(TrustManifestError::InvalidFieldType {
            field: "trustSchema",
            expected: "an object"
        })
    );
}

#[test]
fn rejects_non_array_provenance_and_attestations() {
    assert_eq!(
        TrustManifest::new(json!({"identity": "did:web:example.com", "provenance": "x"})),
        Err(TrustManifestError::InvalidFieldType {
            field: "provenance",
            expected: "an array"
        })
    );
    assert_eq!(
        TrustManifest::new(json!({"identity": "did:web:example.com", "attestations": {}})),
        Err(TrustManifestError::InvalidFieldType {
            field: "attestations",
            expected: "an array"
        })
    );
}

#[test]
fn rejects_non_string_signature() {
    assert_eq!(
        TrustManifest::new(json!({"identity": "did:web:example.com", "signature": 123})),
        Err(TrustManifestError::InvalidFieldType {
            field: "signature",
            expected: "a string"
        })
    );
}
