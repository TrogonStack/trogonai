use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CustomExtension, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SerialNumber,
};
use time::OffsetDateTime;
use x509_parser::pem::Pem;

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

    let ee = ee_params
        .signed_by(&ee_key, &Issuer::from_params(&ca_params, &ca_key))
        .expect("ee");
    let leaf_pem = ee.pem();
    let anchors = ca.pem();

    let v = X509MtlsVerifier::new(TrustAnchorPem::new(anchors));
    let account = AudienceAccount::new("acct-1");
    let claims = v.verify(&ClientCertPem::new(leaf_pem), &account).await.expect("verify");
    assert_eq!(claims.claims().aud.as_str(), "acct-1");
    assert!(claims.claims().sub.as_str().contains("test-service"));
    assert!(!claims.claims().caller_id.as_str().contains('.'));
    assert_eq!(
        claims.claims().principal_kind,
        trogon_identity_types::PrincipalKind::Service
    );
}

#[tokio::test]
async fn rejects_wrong_anchor() {
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

    let ee_key = KeyPair::generate().expect("ee");
    let ee = CertificateParams::default()
        .signed_by(&ee_key, &Issuer::from_params(&ca_params, &ca_key))
        .expect("ee");

    let v = X509MtlsVerifier::new(TrustAnchorPem::new(unrelated_ca.pem()));
    let err = v
        .verify(&ClientCertPem::new(ee.pem()), &AudienceAccount::new("a"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::MtlsCertificate(CertificateVerificationError::Path(
            webpki::Error::UnknownIssuer
        )))
    ));
}

#[test]
fn rejects_pem_with_no_certificate_block() {
    // A PEM file with no CERTIFICATE labels causes chain_ders to return an empty
    // vector error.
    let pem = "-----BEGIN PUBLIC KEY-----\nYWJj\n-----END PUBLIC KEY-----\n";
    let err = X509MtlsVerifier::chain_ders(pem).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn parse_cas_skips_non_certificate_pem_labels() {
    // A bundle where all entries are not CERTIFICATE labels: parse_cas must
    // skip them all and then return an empty-bundle error.
    let bundle = "-----BEGIN PUBLIC KEY-----\nYWJj\n-----END PUBLIC KEY-----\n";
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
    let ee = ee_params
        .signed_by(&ee_key, &Issuer::from_params(&ca_params, &ca_key))
        .expect("ee");

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
        AuthCalloutError::CredentialVerification(CredentialError::MtlsCertificate(CertificateVerificationError::Path(
            webpki::Error::CaUsedAsEndEntity
        )))
    ));
}

#[tokio::test]
async fn verifies_cert_that_is_itself_a_trust_anchor() {
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
    assert_eq!(claims.claims().aud.as_str(), "acct");
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

fn params(common_name: &str, is_ca: IsCa) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = is_ca;
    params
}

fn root() -> (Certificate, CertificateParams, KeyPair) {
    let key = KeyPair::generate().unwrap();
    let mut params = params("root", IsCa::Ca(BasicConstraints::Unconstrained));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key).unwrap();
    (cert, params, key)
}

fn root_without_key_cert_sign() -> (Certificate, CertificateParams, KeyPair) {
    let key = KeyPair::generate().unwrap();
    let mut params = params("invalid-root-usage", IsCa::Ca(BasicConstraints::Unconstrained));
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.not_after = OffsetDateTime::now_utc() + time::Duration::seconds(30);
    let cert = params.self_signed(&key).unwrap();
    (cert, params, key)
}

fn signed(
    params: &CertificateParams,
    issuer_params: &CertificateParams,
    issuer_key: &KeyPair,
) -> (Certificate, KeyPair) {
    let key = KeyPair::generate().unwrap();
    let cert = params
        .signed_by(&key, &Issuer::from_params(issuer_params, issuer_key))
        .unwrap();
    (cert, key)
}

fn assert_refused(chain: &[&Certificate], anchors: &Certificate) {
    let pem = chain.iter().map(|cert| cert.pem()).collect::<String>();
    X509MtlsVerifier::new(TrustAnchorPem::new(anchors.pem()))
        .verify_sync(
            &ClientCertPem::new(pem),
            &AudienceAccount::new("tenant"),
            OffsetDateTime::now_utc(),
        )
        .expect_err("invalid client certificate must not authenticate");
}

#[test]
fn rejects_non_ca_intermediate() {
    let (root, root_params, root_key) = root();
    let issuer_params = params("ordinary-client", IsCa::ExplicitNoCa);
    let (issuer, issuer_key) = signed(&issuer_params, &root_params, &root_key);
    let (leaf, _) = signed(&params("forged-client", IsCa::NoCa), &issuer_params, &issuer_key);
    assert_refused(&[&leaf, &issuer], &root);
}

#[test]
fn rejects_intermediate_without_key_cert_sign() {
    let (root, root_params, root_key) = root();
    let mut issuer_params = params("intermediate", IsCa::Ca(BasicConstraints::Unconstrained));
    issuer_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let (issuer, issuer_key) = signed(&issuer_params, &root_params, &root_key);
    let (leaf, _) = signed(&params("client", IsCa::NoCa), &issuer_params, &issuer_key);
    let error = X509MtlsVerifier::new(TrustAnchorPem::new(root.pem()))
        .verify_sync(
            &ClientCertPem::new(leaf.pem() + &issuer.pem()),
            &AudienceAccount::new("tenant"),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        AuthCalloutError::CredentialVerification(CredentialError::MtlsCertificate(
            CertificateVerificationError::IssuerKeyUsage
        ))
    ));
}

#[test]
fn a_trust_anchor_without_certificate_signing_usage_cannot_authorize_a_leaf() {
    let (root, root_params, root_key) = root_without_key_cert_sign();
    let (leaf, _) = signed(&params("client", IsCa::NoCa), &root_params, &root_key);
    let error = X509MtlsVerifier::new(TrustAnchorPem::new(root.pem()))
        .verify_sync(
            &ClientCertPem::new(leaf.pem()),
            &AudienceAccount::new("tenant"),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        AuthCalloutError::CredentialVerification(CredentialError::MtlsCertificate(CertificateVerificationError::Path(
            webpki::Error::UnknownIssuer
        )))
    ));
}

fn assert_mixed_ca_bundle_accepts_valid_chain(bundle: impl Fn(&Certificate, &Certificate) -> String) {
    for (anchor_seconds, intermediate_seconds) in [(300, 600), (600, 300)] {
        let now = OffsetDateTime::now_utc();
        let (_, mut root_params, root_key) = root();
        root_params.not_after = now + time::Duration::seconds(anchor_seconds);
        let root = root_params.self_signed(&root_key).unwrap();
        let (ineligible, _, _) = root_without_key_cert_sign();
        let mut intermediate_params = params("intermediate", IsCa::Ca(BasicConstraints::Constrained(0)));
        intermediate_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        intermediate_params.not_after = now + time::Duration::seconds(intermediate_seconds);
        let (intermediate, intermediate_key) = signed(&intermediate_params, &root_params, &root_key);
        let mut leaf_params = params("client", IsCa::ExplicitNoCa);
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        leaf_params.not_after = now + time::Duration::seconds(900);
        let (leaf, _) = signed(&leaf_params, &intermediate_params, &intermediate_key);
        let verified = X509MtlsVerifier::new(TrustAnchorPem::new(bundle(&root, &ineligible)))
            .verify_sync(
                &ClientCertPem::new(leaf.pem() + &intermediate.pem()),
                &AudienceAccount::new("tenant"),
                now,
            )
            .expect("an unrelated ineligible CA must not poison a valid trust path");
        assert_eq!(
            verified.expires_at().unix_seconds(),
            now.unix_timestamp() + anchor_seconds.min(intermediate_seconds),
            "source validity must come from the selected certificate path"
        );
    }
}

#[test]
fn mixed_ca_bundle_accepts_valid_chain_after_ineligible_anchor() {
    assert_mixed_ca_bundle_accepts_valid_chain(|eligible, ineligible| ineligible.pem() + &eligible.pem());
}

#[test]
fn mixed_ca_bundle_accepts_valid_chain_before_ineligible_anchor() {
    assert_mixed_ca_bundle_accepts_valid_chain(|eligible, ineligible| eligible.pem() + &ineligible.pem());
}

#[test]
fn mixed_ca_bundle_cannot_authorize_a_leaf_signed_only_by_an_ineligible_anchor() {
    let (eligible, _, _) = root();
    let (ineligible, ineligible_params, ineligible_key) = root_without_key_cert_sign();
    let (leaf, _) = signed(&params("client", IsCa::NoCa), &ineligible_params, &ineligible_key);
    for bundle in [eligible.pem() + &ineligible.pem(), ineligible.pem() + &eligible.pem()] {
        X509MtlsVerifier::new(TrustAnchorPem::new(bundle))
            .verify_sync(
                &ClientCertPem::new(leaf.pem()),
                &AudienceAccount::new("tenant"),
                OffsetDateTime::now_utc(),
            )
            .expect_err("an ineligible CA cannot authorize a client even beside an eligible root");
    }
}

#[test]
fn empty_subject_certificates_keep_distinct_stable_legacy_identities() {
    let (root, root_params, root_key) = root();
    let mut leaf_params = CertificateParams::default();
    leaf_params.distinguished_name = DistinguishedName::new();
    leaf_params.subject_alt_names = vec![rcgen::SanType::URI(
        "spiffe://legacy.example/workload".try_into().unwrap(),
    )];
    let (first, _) = signed(&leaf_params, &root_params, &root_key);
    let (second, _) = signed(&leaf_params, &root_params, &root_key);
    let verifier = X509MtlsVerifier::new(TrustAnchorPem::new(root.pem()));
    let account = AudienceAccount::new("tenant");
    let verify = |certificate: &Certificate| {
        verifier
            .verify_sync(
                &ClientCertPem::new(certificate.pem()),
                &account,
                OffsetDateTime::now_utc(),
            )
            .unwrap()
    };
    let first_identity = verify(&first);
    let repeated = verify(&first);
    let second_identity = verify(&second);
    assert!(first_identity.claims().sub.as_str().starts_with("mtls|"));
    assert_eq!(first_identity.claims().sub, repeated.claims().sub);
    assert_eq!(first_identity.claims().caller_id, repeated.claims().caller_id);
    assert_ne!(first_identity.claims().sub, second_identity.claims().sub);
    assert_ne!(first_identity.claims().caller_id, second_identity.claims().caller_id);
}

#[test]
fn malformed_trust_anchor_pem_cannot_authenticate_a_valid_client() {
    let (_, root_params, root_key) = root();
    let (leaf, _) = signed(&params("client", IsCa::NoCa), &root_params, &root_key);
    let verifier = X509MtlsVerifier::new(TrustAnchorPem::new(
        "-----BEGIN CERTIFICATE-----\ninvalid-base64!\n-----END CERTIFICATE-----",
    ));
    assert!(matches!(
        verifier.verify_sync(
            &ClientCertPem::new(leaf.pem()),
            &AudienceAccount::new("tenant"),
            OffsetDateTime::now_utc(),
        ),
        Err(AuthCalloutError::CredentialVerification(
            CredentialError::InvalidCredentials(_)
        ))
    ));
}

#[test]
fn rejects_intermediate_exceeding_path_length() {
    let (root, root_params, root_key) = root();
    let first_params = params("first-intermediate", IsCa::Ca(BasicConstraints::Constrained(0)));
    let (first, first_key) = signed(&first_params, &root_params, &root_key);
    let second_params = params("second-intermediate", IsCa::Ca(BasicConstraints::Unconstrained));
    let (second, second_key) = signed(&second_params, &first_params, &first_key);
    let (leaf, _) = signed(&params("client", IsCa::NoCa), &second_params, &second_key);
    assert_refused(&[&leaf, &second, &first], &root);
}

#[test]
fn rejects_leaf_for_server_auth_only() {
    let (root, root_params, root_key) = root();
    let mut leaf_params = params("server", IsCa::NoCa);
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let (leaf, _) = signed(&leaf_params, &root_params, &root_key);
    assert_refused(&[&leaf], &root);
}

#[test]
fn rejects_leaf_with_unknown_critical_extension() {
    let (root, root_params, root_key) = root();
    let mut leaf_params = params("client", IsCa::NoCa);
    let mut extension = CustomExtension::from_oid_content(&[1, 2, 3, 4], vec![0x05, 0x00]);
    extension.set_criticality(true);
    leaf_params.custom_extensions.push(extension);
    let (leaf, _) = signed(&leaf_params, &root_params, &root_key);
    assert_refused(&[&leaf], &root);
}

#[test]
fn rejects_pinned_end_entity_as_issuer() {
    let (_, root_params, root_key) = root();
    let issuer_params = params("pinned-client", IsCa::ExplicitNoCa);
    let (issuer, issuer_key) = signed(&issuer_params, &root_params, &root_key);
    let (leaf, _) = signed(&params("forged-client", IsCa::NoCa), &issuer_params, &issuer_key);
    assert_refused(&[&leaf, &issuer], &issuer);
}

#[test]
fn rejects_reissued_certificate_using_pinned_end_entity_key() {
    let key = KeyPair::generate().unwrap();
    let mut original_params = params("pinned-client", IsCa::ExplicitNoCa);
    original_params.serial_number = Some(SerialNumber::from(1u64));
    let original = original_params.self_signed(&key).unwrap();
    let mut reissued_params = original_params;
    reissued_params.serial_number = Some(SerialNumber::from(2u64));
    let reissued = reissued_params.self_signed(&key).unwrap();
    assert_refused(&[&reissued], &original);
}

#[test]
fn rejects_pinned_server_auth_only_certificate() {
    let key = KeyPair::generate().unwrap();
    let mut params = params("pinned-server", IsCa::ExplicitNoCa);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let cert = params.self_signed(&key).unwrap();
    assert_refused(&[&cert], &cert);
}

#[test]
fn verifies_valid_intermediate_chain() {
    let (root, root_params, root_key) = root();
    let mut issuer_params = params("intermediate", IsCa::Ca(BasicConstraints::Constrained(0)));
    issuer_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let (issuer, issuer_key) = signed(&issuer_params, &root_params, &root_key);
    let mut leaf_params = params("client", IsCa::ExplicitNoCa);
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let (leaf, _) = signed(&leaf_params, &issuer_params, &issuer_key);
    let chain = leaf.pem() + &issuer.pem();
    let claims = X509MtlsVerifier::new(TrustAnchorPem::new(root.pem()))
        .verify_sync(
            &ClientCertPem::new(chain),
            &AudienceAccount::new("tenant"),
            OffsetDateTime::now_utc(),
        )
        .expect("valid client chain");
    assert_eq!(claims.claims().sub.as_str(), "CN=client");
}

#[test]
fn verifies_exact_pinned_ca_signed_end_entity() {
    let (_, root_params, root_key) = root();
    let (leaf, _) = signed(&params("pinned-client", IsCa::ExplicitNoCa), &root_params, &root_key);
    let claims = X509MtlsVerifier::new(TrustAnchorPem::new(leaf.pem()))
        .verify_sync(
            &ClientCertPem::new(leaf.pem()),
            &AudienceAccount::new("tenant"),
            OffsetDateTime::now_utc(),
        )
        .expect("exact certificate pin");
    assert_eq!(claims.claims().sub.as_str(), "CN=pinned-client");
}

#[test]
fn rejects_pinned_leaf_with_unknown_critical_extension() {
    let key = KeyPair::generate().unwrap();
    let mut params = params("pinned-client", IsCa::ExplicitNoCa);
    let mut extension = CustomExtension::from_oid_content(&[1, 2, 3, 4], vec![0x05, 0x00]);
    extension.set_criticality(true);
    params.custom_extensions.push(extension);
    let cert = params.self_signed(&key).unwrap();
    assert_refused(&[&cert], &cert);
}

#[test]
fn rejects_pinned_leaf_not_permitted_to_sign() {
    let key = KeyPair::generate().unwrap();
    let mut params = params("pinned-client", IsCa::ExplicitNoCa);
    params.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
    let cert = params.self_signed(&key).unwrap();
    assert_refused(&[&cert], &cert);
}
