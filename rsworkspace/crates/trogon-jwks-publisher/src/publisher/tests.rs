use super::*;

fn empty_set() -> JwkSet {
    JwkSet { keys: vec![] }
}

#[test]
fn builder_accepts_known_dwk_filenames() {
    for dwk in known_dwk_filenames() {
        let cfg = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
            .with_jwk_set(dwk, empty_set())
            .unwrap_or_else(|_| panic!("dwk {dwk} should be accepted"))
            .build();
        assert!(cfg.entries.contains_key(dwk));
    }
}

#[test]
fn builder_rejects_unknown_dwk_filename() {
    let err = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set("nonsense.json", empty_set())
        .unwrap_err();
    assert!(matches!(err, PublisherError::UnknownDwk(name) if name == "nonsense.json"));
}

#[test]
fn builder_rejects_duplicate_dwk_registration() {
    let err = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set(DWK_AGENT, empty_set())
        .expect("first registration ok")
        .with_jwk_set(DWK_AGENT, empty_set())
        .unwrap_err();
    assert!(matches!(err, PublisherError::DuplicateDwk(name) if name == DWK_AGENT));
}

#[test]
fn builder_allows_empty_jwk_set() {
    let cfg = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set(DWK_AGENT, empty_set())
        .expect("empty jwk set is legitimate")
        .build();
    assert_eq!(cfg.entries.get(DWK_AGENT).expect("present").keys.len(), 0);
}

const TEST_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";

#[test]
fn jwk_from_ec_pkcs8_pem_produces_expected_shape() {
    let jwk = jwk_from_ec_pkcs8_pem(TEST_PEM, "k1").expect("valid pem parses");
    assert_eq!(jwk.common.key_id.as_deref(), Some("k1"));
    match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) => {
            assert_eq!(ec.curve, EllipticCurve::P256);
            assert!(!ec.x.is_empty());
            assert!(!ec.y.is_empty());
        }
        other => panic!("expected EC algorithm params, got {other:?}"),
    }
}

#[test]
fn jwk_from_ec_pkcs8_pem_rejects_garbage_pem() {
    let err = jwk_from_ec_pkcs8_pem("not a pem", "k1").unwrap_err();
    assert!(matches!(err, PublisherError::InvalidPem { kid, .. } if kid == "k1"));
}

#[test]
fn builder_with_ec_pkcs8_pem_registers_single_key_set() {
    let cfg = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_ec_pkcs8_pem(DWK_AGENT, TEST_PEM, "k1")
        .expect("valid pem registers")
        .build();
    let set = cfg.entries.get(DWK_AGENT).expect("present");
    assert_eq!(set.keys.len(), 1);
}

#[test]
fn cache_max_age_renders_header_value() {
    let max_age = CacheMaxAge::new(300);
    assert_eq!(max_age.header_value(), "max-age=300");
    assert_eq!(max_age.as_secs(), 300);
}
