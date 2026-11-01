use super::*;
use crate::source::plaid::test_support::TestKeyPair;

#[test]
fn accepts_an_es256_p256_key() {
    let pair = TestKeyPair::generate("kid-1");
    let key = PlaidVerificationKey::from_jwk(&pair.jwk(None), &pair.key_id()).unwrap();
    assert_eq!(key.key_id().as_str(), "kid-1");
}

#[test]
fn key_without_expiry_never_expires() {
    let pair = TestKeyPair::generate("kid-1");
    let key = pair.verification_key(None);
    assert!(!key.is_expired_at(UNIX_EPOCH + Duration::from_secs(u32::MAX as u64)));
}

#[test]
fn expiry_is_inclusive_of_the_expiry_instant() {
    let pair = TestKeyPair::generate("kid-1");
    let key = pair.verification_key(Some(1_000));
    assert!(!key.is_expired_at(UNIX_EPOCH + Duration::from_secs(999)));
    assert!(key.is_expired_at(UNIX_EPOCH + Duration::from_secs(1_000)));
    assert!(key.is_expired_at(UNIX_EPOCH + Duration::from_secs(1_001)));
}

#[test]
fn rejects_a_key_id_that_differs_from_the_request() {
    let pair = TestKeyPair::generate("kid-1");
    let requested = PlaidKeyId::new("kid-2").unwrap();
    let err = PlaidVerificationKey::from_jwk(&pair.jwk(None), &requested).unwrap_err();
    assert!(matches!(err, PlaidVerificationKeyError::KeyIdMismatch { .. }));
    assert_eq!(
        err.to_string(),
        "returned key id kid-1 does not match requested key id kid-2"
    );
}

#[test]
fn rejects_an_empty_returned_key_id() {
    let pair = TestKeyPair::generate("kid-1");
    let mut jwk = pair.jwk(None);
    jwk.kid = String::new();
    let err = PlaidVerificationKey::from_jwk(&jwk, &pair.key_id()).unwrap_err();
    assert!(matches!(err, PlaidVerificationKeyError::InvalidKeyId(_)));
}

#[test]
fn rejects_unsupported_parameters() {
    let pair = TestKeyPair::generate("kid-1");
    for (alg, kty, crv) in [
        ("RS256", "EC", "P-256"),
        ("ES256", "RSA", "P-256"),
        ("ES256", "EC", "P-384"),
    ] {
        let mut jwk = pair.jwk(None);
        jwk.alg = alg.to_owned();
        jwk.kty = kty.to_owned();
        jwk.crv = crv.to_owned();
        let err = PlaidVerificationKey::from_jwk(&jwk, &pair.key_id()).unwrap_err();
        assert!(matches!(err, PlaidVerificationKeyError::UnsupportedParameters { .. }));
        assert_eq!(
            err.to_string(),
            format!("unsupported key parameters: alg {alg}, kty {kty}, crv {crv}")
        );
    }
}

#[test]
fn rejects_coordinates_that_are_not_base64url() {
    let pair = TestKeyPair::generate("kid-1");
    let mut jwk = pair.jwk(None);
    jwk.x = "!!!not-base64!!!".to_owned();
    let err = PlaidVerificationKey::from_jwk(&jwk, &pair.key_id()).unwrap_err();
    assert!(matches!(err, PlaidVerificationKeyError::InvalidCoordinates(_)));
}

#[test]
fn jwk_deserializes_the_plaid_response_shape() {
    let pair = TestKeyPair::generate("kid-1");
    let body = pair.jwk_json(Some(42));
    let jwk: PlaidJwk = serde_json::from_value(body["key"].clone()).unwrap();
    assert_eq!(jwk.kid, "kid-1");
    assert_eq!(jwk.expired_at, Some(42));

    let body = pair.jwk_json(None);
    let jwk: PlaidJwk = serde_json::from_value(body["key"].clone()).unwrap();
    assert_eq!(jwk.expired_at, None);
}
