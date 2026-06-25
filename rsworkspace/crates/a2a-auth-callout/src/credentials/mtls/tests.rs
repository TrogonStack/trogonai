use time::OffsetDateTime;

use super::*;

#[test]
fn rejects_empty_trust_bundle() {
    let empty: Vec<Pem> = Vec::new();
    let err = X509MtlsVerifier::parse_cas(&empty).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
    let _ = X509MtlsVerifier::new(TrustAnchorPem::new(""));
}

#[tokio::test]
async fn verifies_rcgen_chain() {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "test-ca");
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    let ca = ca_params.self_signed(&ca_key).expect("ca");

    let ee_key = KeyPair::generate().expect("ee key");
    let mut ee_dn = DistinguishedName::new();
    ee_dn.push(DnType::CommonName, "test-service");
    let mut ee_params = CertificateParams::default();
    ee_params.distinguished_name = ee_dn;

    let ee = ee_params.signed_by(&ee_key, &ca, &ca_key).expect("ee");
    let leaf_pem = ee.pem();
    let anchors = ca.pem();

    let v = X509MtlsVerifier::new(TrustAnchorPem::new(anchors));
    let account = AudienceAccount::new("acct-1");
    let claims = v.verify(&ClientCertPem::new(leaf_pem), &account).await.expect("verify");
    assert_eq!(claims.aud.as_str(), "acct-1");
    assert!(claims.sub.as_str().contains("test-service"));
    assert!(!claims.caller_id.as_str().contains('.'));
}

#[tokio::test]
async fn rejects_wrong_anchor() {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

    let unrelated_key = KeyPair::generate().expect("k");
    let mut unrelated_dn = DistinguishedName::new();
    unrelated_dn.push(DnType::CommonName, "other-ca");
    let mut unrelated_params = CertificateParams::default();
    unrelated_params.distinguished_name = unrelated_dn;
    unrelated_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let unrelated_ca = unrelated_params.self_signed(&unrelated_key).expect("uca");

    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "real-ca");
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).expect("ca");

    let ee_key = KeyPair::generate().expect("ee");
    let ee = CertificateParams::default()
        .signed_by(&ee_key, &ca, &ca_key)
        .expect("ee");

    let v = X509MtlsVerifier::new(TrustAnchorPem::new(unrelated_ca.pem()));
    let err = v
        .verify(&ClientCertPem::new(ee.pem()), &AudienceAccount::new("a"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn rejects_pem_with_no_certificate_block() {
    // A PEM file with no CERTIFICATE labels causes chain_ders to return an empty
    // vector error.
    let pem = "-----BEGIN PRIVATE KEY-----\nYWJj\n-----END PRIVATE KEY-----\n";
    let err = X509MtlsVerifier::chain_ders(pem).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn parse_cas_skips_non_certificate_pem_labels() {
    use x509_parser::pem::Pem;
    // A bundle where all entries are not CERTIFICATE labels: parse_cas must
    // skip them all and then return an empty-bundle error.
    let bundle = "-----BEGIN PRIVATE KEY-----\nYWJj\n-----END PRIVATE KEY-----\n";
    let pems: Vec<Pem> = Pem::iter_from_buffer(bundle.as_bytes())
        .filter_map(|r| r.ok())
        .collect();
    let err = X509MtlsVerifier::parse_cas(&pems).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn rejects_expired_certificate() {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "test-ca");
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).expect("ca");

    let ee_key = KeyPair::generate().expect("ee key");
    let mut ee_dn = DistinguishedName::new();
    ee_dn.push(DnType::CommonName, "test-service");
    let mut ee_params = CertificateParams::default();
    ee_params.distinguished_name = ee_dn;
    let ee = ee_params.signed_by(&ee_key, &ca, &ca_key).expect("ee");

    let v = X509MtlsVerifier::new(TrustAnchorPem::new(ca.pem()));
    // Use a `now` far in the past so the cert is outside its validity window.
    let past = OffsetDateTime::from_unix_timestamp(0).unwrap();
    let err = v
        .verify_sync(&ClientCertPem::new(ee.pem()), &AudienceAccount::new("acct"), past)
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn rejects_ca_cert_presented_as_leaf() {
    use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "test-ca");
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).expect("ca");

    // Present the CA cert itself as the client cert.
    let v = X509MtlsVerifier::new(TrustAnchorPem::new(ca.pem()));
    let now = OffsetDateTime::now_utc();
    let err = v
        .verify_sync(&ClientCertPem::new(ca.pem()), &AudienceAccount::new("acct"), now)
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn verifies_cert_that_is_itself_a_trust_anchor() {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};

    // A self-signed end-entity cert. When the same cert is placed in both the
    // client PEM and the trust anchor bundle it should short-circuit to
    // trusted = true via the "current cert is itself a configured trust anchor"
    // branch.
    let key = KeyPair::generate().expect("key");
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "self-signed-ee");
    let mut params = CertificateParams::default();
    params.distinguished_name = dn;
    let cert = params.self_signed(&key).expect("cert");

    let v = X509MtlsVerifier::new(TrustAnchorPem::new(cert.pem()));
    let now = OffsetDateTime::now_utc();
    let claims = v
        .verify_sync(&ClientCertPem::new(cert.pem()), &AudienceAccount::new("acct"), now)
        .expect("should be trusted");
    assert_eq!(claims.aud.as_str(), "acct");
}

#[test]
fn client_cert_pem_accessors_work() {
    let pem = ClientCertPem::new("-----BEGIN CERTIFICATE-----\nYWJj\n-----END CERTIFICATE-----");
    assert!(pem.as_str().contains("CERTIFICATE"));
    let pem2 = pem.clone();
    assert_eq!(pem, pem2);

    let anchor = TrustAnchorPem::new("bundle");
    assert_eq!(anchor.as_str(), "bundle");
    let anchor2 = anchor.clone();
    assert_eq!(anchor, anchor2);
}
