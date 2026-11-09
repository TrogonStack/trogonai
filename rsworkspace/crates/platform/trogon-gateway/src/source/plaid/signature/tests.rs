use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;

use super::*;
use crate::source::plaid::test_support::{TestKeyPair, hs256_token};

const NOW_SECS: u64 = 1_700_000_000;
const TOLERANCE: Duration = Duration::from_secs(300);
const BODY: &[u8] = br#"{"webhook_type":"TRANSACTIONS","webhook_code":"SYNC_UPDATES_AVAILABLE"}"#;

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

#[test]
fn key_id_reads_the_kid_of_an_es256_token() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS, BODY);
    assert_eq!(key_id(&token).unwrap().as_str(), "kid-1");
}

#[test]
fn key_id_rejects_a_malformed_token() {
    let err = key_id("not-a-jwt").unwrap_err();
    assert!(matches!(err, SignatureError::MalformedHeader(_)));
}

#[test]
fn key_id_rejects_a_non_es256_algorithm() {
    let token = hs256_token("kid-1", NOW_SECS, BODY);
    let err = key_id(&token).unwrap_err();
    assert!(matches!(err, SignatureError::UnsupportedAlgorithm(Algorithm::HS256)));
    assert_eq!(err.to_string(), "unsupported algorithm HS256, expected ES256");
}

#[test]
fn key_id_rejects_a_token_without_kid() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token_without_key_id(NOW_SECS, BODY);
    assert!(matches!(key_id(&token).unwrap_err(), SignatureError::MissingKeyId));
}

#[test]
fn key_id_rejects_an_empty_kid() {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"ES256","typ":"JWT","kid":""}"#);
    let token = format!("{header}.e30.c2ln");

    assert!(matches!(key_id(&token).unwrap_err(), SignatureError::InvalidKeyId(_)));
}

#[test]
fn verify_accepts_a_valid_token_and_returns_its_delivery_id() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS, BODY);

    let delivery_id = verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).unwrap();

    assert_eq!(delivery_id, PlaidDeliveryId::of(&token));
}

#[test]
fn verify_gives_tokens_over_the_same_body_distinct_delivery_ids() {
    let pair = TestKeyPair::generate("kid-1");
    let first = pair.token(NOW_SECS - 1, BODY);
    let second = pair.token(NOW_SECS, BODY);
    let key = pair.verification_key(None);

    let first_id = verify(&first, &key, BODY, now(), TOLERANCE).unwrap();
    let second_id = verify(&second, &key, BODY, now(), TOLERANCE).unwrap();

    assert_ne!(first_id, second_id);
}

#[test]
fn verify_rejects_a_signature_from_another_key() {
    let signer = TestKeyPair::generate("kid-1");
    let other = TestKeyPair::generate("kid-1");
    let token = signer.token(NOW_SECS, BODY);

    let err = verify(&token, &other.verification_key(None), BODY, now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::InvalidToken(_)));
}

#[test]
fn verify_rejects_a_tampered_payload() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS, BODY);
    let other = pair.token(NOW_SECS + 1, BODY);
    let mut parts: Vec<&str> = token.split('.').collect();
    let other_parts: Vec<&str> = other.split('.').collect();
    parts[1] = other_parts[1];
    let forged = parts.join(".");

    let err = verify(&forged, &pair.verification_key(None), BODY, now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::InvalidToken(_)));
}

#[test]
fn verify_rejects_a_stale_issued_at() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS - 301, BODY);

    let err = verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::IssuedAtOutsideTolerance(_)));
}

#[test]
fn verify_accepts_issued_at_exactly_on_the_tolerance_boundary() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS - 300, BODY);

    assert!(verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).is_ok());
}

#[test]
fn verify_rejects_an_issued_at_too_far_in_the_future() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS + 301, BODY);

    let err = verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::IssuedAtOutsideTolerance(_)));
}

#[test]
fn verify_accepts_a_slightly_future_issued_at() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS + 30, BODY);

    assert!(verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).is_ok());
}

#[test]
fn verify_rejects_a_body_that_differs_from_the_claim() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS, BODY);

    let err = verify(&token, &pair.verification_key(None), b"{}", now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::BodyHashMismatch));
}

#[test]
fn verify_rejects_whitespace_changes_to_the_body() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.token(NOW_SECS, b"{\"a\":1}");

    let err = verify(&token, &pair.verification_key(None), b"{\"a\": 1}", now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::BodyHashMismatch));
}

#[test]
fn verify_rejects_a_non_es256_token_even_with_a_valid_key() {
    let pair = TestKeyPair::generate("kid-1");
    let token = hs256_token("kid-1", NOW_SECS, BODY);

    let err = verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::InvalidToken(_)));
}

#[test]
fn verify_rejects_claims_missing_the_body_hash() {
    let pair = TestKeyPair::generate("kid-1");
    let token = pair.sign(&json!({"iat": NOW_SECS}));

    let err = verify(&token, &pair.verification_key(None), BODY, now(), TOLERANCE).unwrap_err();

    assert!(matches!(err, SignatureError::InvalidToken(_)));
}
