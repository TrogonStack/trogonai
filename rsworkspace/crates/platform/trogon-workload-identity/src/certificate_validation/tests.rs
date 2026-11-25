use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose, SanType,
    string::Ia5String,
};

use super::*;
use crate::test_support::TestCa;
use crate::tests::PEER_ID;

fn conforming_params(lifetime: Duration) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.is_ca = IsCa::ExplicitNoCa;
    params
        .subject_alt_names
        .push(SanType::URI(Ia5String::try_from(PEER_ID).expect("URI")));
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::seconds(1);
    params.not_after = now + time::Duration::try_from(lifetime).expect("lifetime");
    params
}

fn profile_of(params: CertificateParams) -> Result<(), MaterialError> {
    let certificate = TestCa::new()
        .issue_with_params(PEER_ID, params)
        .certificates()
        .remove(0);
    leaf_profile(&certificate, SystemTime::now()).map(|_| ())
}

#[test]
fn a_trust_anchor_must_consume_its_entire_der_input() {
    let ca = TestCa::new();
    let identity = ca.issue(PEER_ID, Duration::from_secs(600));
    let chain = identity.certificates();
    let root = ca.root();
    let now = SystemTime::now();
    assert!(verify_chain(&chain, std::slice::from_ref(&root), now, CertificateUse::Client).is_ok());
    for suffix in [vec![0], root.to_vec()] {
        let mut ambiguous = root.to_vec();
        ambiguous.extend_from_slice(&suffix);
        assert_eq!(
            verify_chain(&chain, &[CertificateDer::from(ambiguous)], now, CertificateUse::Client),
            Err(MaterialError::Certificate)
        );
    }
}

#[test]
fn a_conforming_leaf_is_accepted() {
    assert_eq!(profile_of(conforming_params(Duration::from_secs(600))), Ok(()));
}

#[test]
fn key_cert_sign_alone_refuses_a_leaf() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn crl_sign_alone_refuses_a_leaf() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.key_usages.push(KeyUsagePurpose::CrlSign);
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn a_ca_leaf_alone_is_refused() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn a_leaf_without_digital_signature_alone_is_refused() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn client_auth_only_extended_key_usage_alone_is_refused() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn server_auth_only_extended_key_usage_alone_is_refused() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn a_leaf_without_extended_key_usage_is_accepted() {
    let mut params = conforming_params(Duration::from_secs(600));
    params.extended_key_usages = Vec::new();
    assert_eq!(profile_of(params), Ok(()));
}

#[test]
fn an_expired_leaf_alone_is_refused() {
    let mut params = conforming_params(Duration::from_secs(600));
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::seconds(120);
    params.not_after = now - time::Duration::seconds(60);
    assert_eq!(profile_of(params), Err(MaterialError::Profile));
}

#[test]
fn a_lifetime_over_the_cap_alone_is_refused() {
    let cap = MAX_SVID_LIFETIME;
    assert_eq!(profile_of(conforming_params(cap - Duration::from_secs(1))), Ok(()));
    assert_eq!(
        profile_of(conforming_params(cap + Duration::from_secs(60))),
        Err(MaterialError::Profile)
    );
}

#[test]
fn the_public_key_profile_check_agrees_with_the_leaf_profile() {
    let mut cases = Vec::new();
    cases.push(conforming_params(Duration::from_secs(600)));
    for usage in [KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign] {
        let mut params = conforming_params(Duration::from_secs(600));
        params.key_usages.push(usage);
        cases.push(params);
    }
    let mut ca = conforming_params(Duration::from_secs(600));
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    cases.push(ca);
    for case in cases {
        let certificate = TestCa::new().issue_with_params(PEER_ID, case).certificates().remove(0);
        assert_eq!(
            verify_leaf_key_profile(&certificate),
            leaf_profile(&certificate, SystemTime::now()).map(|_| ())
        );
    }
}

#[test]
fn the_public_key_profile_check_refuses_unparsable_and_trailing_input() {
    let certificate = TestCa::new()
        .issue(PEER_ID, Duration::from_secs(600))
        .certificates()
        .remove(0);
    let mut trailing = certificate.to_vec();
    trailing.push(0);
    assert_eq!(verify_leaf_key_profile(&trailing), Err(MaterialError::Profile));
    assert_eq!(verify_leaf_key_profile(&[0, 1, 2]), Err(MaterialError::Profile));
    assert_eq!(verify_leaf_key_profile(&certificate), Ok(()));
}

#[test]
fn the_leaf_lifetime_refuses_unparsable_and_trailing_input() {
    let certificate = TestCa::new()
        .issue(PEER_ID, Duration::from_secs(600))
        .certificates()
        .remove(0);
    let mut trailing = certificate.to_vec();
    trailing.push(0);
    assert_eq!(leaf_lifetime(&trailing), Err(MaterialError::Profile));
    assert_eq!(leaf_lifetime(&[0, 1, 2]), Err(MaterialError::Profile));
    assert!(leaf_lifetime(&certificate).unwrap() >= Duration::from_secs(600));
}

#[test]
fn a_zero_or_inverted_validity_span_is_refused() {
    let instant = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("whole second");
    for not_after in [instant, instant - time::Duration::seconds(30)] {
        let mut params = conforming_params(Duration::from_secs(60));
        params.not_before = instant;
        params.not_after = not_after;
        let certificate = TestCa::new()
            .issue_with_params(PEER_ID, params)
            .certificates()
            .remove(0);
        assert_eq!(leaf_lifetime(&certificate), Err(MaterialError::Profile));
        assert_eq!(
            leaf_profile(&certificate, SystemTime::now()).map(|_| ()),
            Err(MaterialError::Profile)
        );
    }
}
