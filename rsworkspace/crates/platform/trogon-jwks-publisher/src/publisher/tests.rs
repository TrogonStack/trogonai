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
    assert_eq!(cfg.entries.get(DWK_AGENT).expect("present").as_jwk_set().keys.len(), 0);
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
    assert_eq!(set.as_jwk_set().keys.len(), 1);
}

struct UnserializableValue;

impl serde::Serialize for UnserializableValue {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom("deliberate encode failure for coverage"))
    }
}

#[test]
fn jwks_response_maps_serialize_failure_to_internal_server_error() {
    let response = jwks_response(&UnserializableValue, &CacheMaxAge::new(60));
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn cache_max_age_renders_header_value() {
    let max_age = CacheMaxAge::new(300);
    assert_eq!(max_age.header_value(), "max-age=300");
    assert_eq!(max_age.as_secs(), 300);
}

fn keyed(kid: &str) -> Jwk {
    jwk_from_ec_pkcs8_pem(TEST_PEM, kid).expect("valid pem")
}

fn unkeyed() -> Jwk {
    let mut jwk = keyed("k1");
    jwk.common.key_id = None;
    jwk
}

#[test]
fn builder_accepts_a_rotation_overlap_with_distinct_key_ids() {
    let cfg = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set(
            DWK_AGENT,
            JwkSet {
                keys: vec![keyed("current"), keyed("previous")],
            },
        )
        .expect("distinct kids stay selectable")
        .build();
    assert_eq!(cfg.entries.get(DWK_AGENT).expect("present").as_jwk_set().keys.len(), 2);
}

#[test]
fn builder_rejects_a_set_that_repeats_a_key_id() {
    let err = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set(
            DWK_AGENT,
            JwkSet {
                keys: vec![keyed("same"), keyed("same")],
            },
        )
        .unwrap_err();
    assert!(
        matches!(
            &err,
            PublisherError::Unpublishable {
                dwk,
                source: UnpublishableJwkSet::DuplicateKeyId { kid },
            } if dwk == DWK_AGENT && kid.as_str() == "same"
        ),
        "{err}"
    );
}

#[test]
fn a_set_that_repeats_a_key_id_cannot_be_built_at_all() {
    // The registrar is not the only way in: the invariant belongs to the type,
    // so converting the set directly refuses it just the same.
    let err = PublishableJwkSet::try_from(JwkSet {
        keys: vec![keyed("same"), keyed("same")],
    })
    .unwrap_err();
    assert!(
        matches!(&err, UnpublishableJwkSet::DuplicateKeyId { kid } if kid.as_str() == "same"),
        "{err}"
    );
}

#[test]
fn builder_rejects_a_multi_key_set_holding_an_unidentified_key() {
    let err = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set(
            DWK_AGENT,
            JwkSet {
                keys: vec![keyed("current"), unkeyed()],
            },
        )
        .unwrap_err();
    assert!(
        matches!(
            &err,
            PublisherError::Unpublishable {
                dwk,
                source: UnpublishableJwkSet::MissingKeyId { keys: 2 },
            } if dwk == DWK_AGENT
        ),
        "{err}"
    );
}

#[test]
fn builder_allows_a_lone_key_without_a_key_id() {
    // `pick_jwk` resolves a one-key set without consulting `kid`, and RFC 7517
    // section 4.5 leaves the member optional, so this shape stays publishable.
    let cfg = JwksPublisherConfigBuilder::new(CacheMaxAge::new(60))
        .with_jwk_set(DWK_AGENT, JwkSet { keys: vec![unkeyed()] })
        .expect("a sole key needs no kid")
        .build();
    assert_eq!(cfg.entries.get(DWK_AGENT).expect("present").as_jwk_set().keys.len(), 1);
}
