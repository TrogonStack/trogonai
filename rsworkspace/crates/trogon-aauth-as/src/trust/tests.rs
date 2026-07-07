use super::*;

#[test]
fn ps_issuer_rejects_empty() {
    assert_eq!(PsIssuer::new("").unwrap_err(), TrustRegistryError::EmptyIssuer);
    assert_eq!(PsIssuer::new("   ").unwrap_err(), TrustRegistryError::EmptyIssuer);
}

#[test]
fn ps_issuer_rejects_non_https() {
    assert_eq!(
        PsIssuer::new("http://ps.example").unwrap_err(),
        TrustRegistryError::NotHttps("http://ps.example".into())
    );
}

#[test]
fn ps_issuer_trims_whitespace() {
    let iss = PsIssuer::new("  https://ps.example  ").unwrap();
    assert_eq!(iss.as_str(), "https://ps.example");
}

#[test]
fn registry_from_entries_fails_loudly_on_invalid_issuer() {
    let err = TrustRegistry::from_entries([("not-https".to_string(), TrustBasisRecord::PreEstablished)]).unwrap_err();
    assert!(matches!(err, TrustRegistryError::NotHttps(_)));
}

#[test]
fn registry_from_entries_accepts_two_distinct_issuers() {
    let registry = TrustRegistry::from_entries([
        ("https://ps-one.example".to_string(), TrustBasisRecord::PreEstablished),
        ("https://ps-two.example".to_string(), TrustBasisRecord::ClaimsOnly),
    ])
    .unwrap();
    assert!(registry.contains("https://ps-one.example"));
    assert!(registry.contains("https://ps-two.example"));
}

#[test]
fn registry_from_entries_fails_loudly_on_duplicate_issuer() {
    let err = TrustRegistry::from_entries([
        ("https://ps.example".to_string(), TrustBasisRecord::PreEstablished),
        ("https://ps.example".to_string(), TrustBasisRecord::ClaimsOnly),
    ])
    .unwrap_err();
    assert_eq!(err, TrustRegistryError::DuplicateIssuer("https://ps.example".into()));
}

#[test]
fn registry_rejects_unknown_ps() {
    let registry =
        TrustRegistry::from_entries([("https://ps.example".to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let err = registry.require("https://unknown-ps.example").unwrap_err();
    assert_eq!(
        err,
        TrustRegistryError::UnknownIssuer("https://unknown-ps.example".into())
    );
    assert!(!registry.contains("https://unknown-ps.example"));
}

#[test]
fn registry_accepts_known_ps_and_reports_basis() {
    let registry =
        TrustRegistry::from_entries([("https://ps.example".to_string(), TrustBasisRecord::Interaction)]).unwrap();
    let trusted = registry.require("https://ps.example").unwrap();
    assert_eq!(trusted.issuer().as_str(), "https://ps.example");
    assert_eq!(trusted.basis().kind(), TrustBasis::Interaction);
    assert!(registry.contains("https://ps.example"));
}

#[test]
fn registry_trust_inserts_dynamically() {
    let mut registry = TrustRegistry::empty();
    assert!(!registry.contains("https://ps.example"));
    registry.trust(PsIssuer::new("https://ps.example").unwrap(), TrustBasisRecord::Payment);
    assert!(registry.contains("https://ps.example"));
    assert_eq!(
        registry.require("https://ps.example").unwrap().basis().kind(),
        TrustBasis::Payment
    );
}

#[test]
fn trust_basis_record_kind_maps_pre_established_and_claims_only() {
    assert_eq!(TrustBasisRecord::PreEstablished.kind(), TrustBasis::PreEstablished);
    assert_eq!(TrustBasisRecord::ClaimsOnly.kind(), TrustBasis::ClaimsOnly);
}
