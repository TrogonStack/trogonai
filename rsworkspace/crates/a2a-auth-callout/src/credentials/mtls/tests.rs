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
