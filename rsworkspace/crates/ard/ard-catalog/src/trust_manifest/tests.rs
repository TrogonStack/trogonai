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

#[test]
fn rejects_a_spiffe_identity_type_with_a_malformed_spiffe_id() {
    let too_long = format!("spiffe://example.com/{}", "a".repeat(2048));
    let trust_domain_too_long = format!("spiffe://{}/agent", "a".repeat(256));
    for identity in [
        trust_domain_too_long.as_str(),
        "spiffe://example.com",
        "did:web:example.com",
        "https://example.com/agent",
        "SPIFFE://example.com/agent",
        "spiffe://",
        "spiffe:///agent",
        "spiffe://Example.com/agent",
        "spiffe://example.com/agent/",
        "spiffe://example.com//agent",
        "spiffe://example.com/agent/..",
        "spiffe://example.com/agent/.",
        "spiffe://example.com/agent?x=1",
        "spiffe://example.com/agent#f",
        "spiffe://example.com:443/agent",
        "spiffe://user@example.com/agent",
        "spiffe://example.com/agent/a b",
        "spiffe://example.com/agent/%41",
        "spiffe://example.com/agent/did:key:x",
        too_long.as_str(),
    ] {
        assert_eq!(
            TrustManifest::new(json!({ "identity": identity, "identityType": "spiffe" })),
            Err(TrustManifestError::InvalidSpiffeIdentity),
            "{identity:?}"
        );
    }
}

#[test]
fn accepts_well_formed_spiffe_ids_and_leaves_other_identity_types_opaque() {
    for identity in [
        "spiffe://example.com/agent/assistant",
        "spiffe://a-b_c.example.com/A/b-c_d.e/1",
    ] {
        assert!(TrustManifest::new(json!({ "identity": identity, "identityType": "spiffe" })).is_ok());
    }
    for (identity, identity_type) in [
        ("not a spiffe id", "other"),
        ("spiffe://Example.com//x", "other"),
        ("did:web:example.com", "did"),
    ] {
        assert!(TrustManifest::new(json!({ "identity": identity, "identityType": identity_type })).is_ok());
    }
    assert!(TrustManifest::new(json!({ "identity": "not a spiffe id" })).is_ok());
}

#[test]
fn rejects_a_non_string_identity() {
    assert_eq!(
        TrustManifest::new(json!({ "identity": 7 })),
        Err(TrustManifestError::InvalidIdentity)
    );
}

#[test]
fn accepts_a_spiffe_id_whose_trust_domain_is_exactly_at_the_length_limit() {
    let identity = format!("spiffe://{}/agent", "a".repeat(255));
    assert!(TrustManifest::new(json!({ "identity": identity, "identityType": "spiffe" })).is_ok());
}
