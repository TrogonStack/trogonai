use pkcs8::EncodePrivateKey as _;
use rand_core::OsRng;

use super::*;
use crate::constants::{MAX_CLOCK_SKEW_SECS, REPLAY_RECORD_LIFETIME_SECS};
use crate::test_support::{challenge_key, challenge_key_from, challenge_key_pem};

const ISSUER: &str = "https://bridge.example";
const JKT: &str = "key-thumbprint";

fn nonces_with(key: ChallengeKey, ttl: u32) -> ServerNonces {
    ServerNonces::new(
        key,
        NonceTtl::new(ttl).expect("ttl in range"),
        NonceRequirement::Required,
    )
}

fn nonces() -> ServerNonces {
    nonces_with(challenge_key("nonce-k1"), 60)
}

fn skew() -> ClockTolerance {
    ClockTolerance::default()
}

fn sign_claims(key: &ChallengeKey, typ: &str, kid: &str, claims: &NonceClaims) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(typ.to_string());
    header.kid = Some(kid.to_string());
    encode(&header, claims, &key.encoding).expect("sign claims")
}

fn claims(iat: i64, exp: i64) -> NonceClaims {
    NonceClaims {
        iss: ISSUER.to_string(),
        jti: "fixed-id".to_string(),
        jkt: JKT.to_string(),
        iat,
        exp,
    }
}

#[test]
fn challenge_kid_is_trimmed() {
    assert_eq!(ChallengeKid::new("  nonce-k1 ").expect("kid").as_str(), "nonce-k1");
}

#[test]
fn challenge_kid_refuses_an_empty_id() {
    assert!(ChallengeKid::new("   ").is_err());
}

#[test]
fn challenge_key_refuses_a_pem_that_is_not_a_private_key() {
    let kid = ChallengeKid::new("nonce-k1").expect("kid");
    let err = ChallengeKey::from_pkcs8_pem("not a pem", kid).expect_err("garbage refused");
    assert!(matches!(err, ChallengeKeyError::Parse(_)), "got {err:?}");
}

#[test]
fn challenge_key_refuses_a_key_on_another_curve() {
    let pem = ed25519_dalek::SigningKey::generate(&mut OsRng)
        .to_pkcs8_pem(pkcs8::LineEnding::LF)
        .expect("ed25519 pem");
    let kid = ChallengeKid::new("nonce-k1").expect("kid");
    let err = ChallengeKey::from_pkcs8_pem(&pem, kid).expect_err("ed25519 refused");
    assert!(matches!(err, ChallengeKeyError::Parse(_)), "got {err:?}");
}

#[test]
fn challenge_key_debug_names_only_its_kid() {
    let rendered = format!("{:?}", challenge_key("nonce-k1"));
    assert!(rendered.contains("nonce-k1"));
    assert!(!rendered.contains("encoding"));
}

#[test]
fn nonce_ttl_accepts_one_second_through_the_ceiling() {
    assert_eq!(NonceTtl::new(1).expect("one second").secs(), 1);
    assert_eq!(
        NonceTtl::new(MAX_SERVER_NONCE_TTL_SECS).expect("ceiling").secs(),
        MAX_SERVER_NONCE_TTL_SECS
    );
}

#[test]
fn nonce_ttl_refuses_zero_and_anything_past_the_ceiling() {
    assert!(NonceTtl::new(0).is_err());
    assert!(NonceTtl::new(MAX_SERVER_NONCE_TTL_SECS + 1).is_err());
}

#[test]
fn nonce_ttl_defaults_to_one_minute() {
    assert_eq!(NonceTtl::default().secs(), 60);
}

#[test]
fn record_of_the_longest_nonce_fits_the_replay_record_lifetime() {
    let longest = nonces_with(challenge_key("nonce-k1"), MAX_SERVER_NONCE_TTL_SECS);
    let widest = ClockTolerance::new(MAX_CLOCK_SKEW_SECS).expect("ceiling");
    assert_eq!(longest.record_ttl_secs(widest), REPLAY_RECORD_LIFETIME_SECS);
    assert_eq!(nonces().record_ttl_secs(skew()), 120);
}

#[test]
fn a_minted_nonce_checks_for_the_key_it_was_issued_to() {
    let nonces = nonces();
    let issued = nonces.mint(ISSUER, JKT, 1000).expect("mint");

    let record = nonces
        .check(issued.nonce.as_str(), ISSUER, JKT, 1000, skew())
        .expect("check");

    assert_eq!(record, issued.record);
    assert!(record.key().starts_with("nonce."));
}

#[test]
fn every_mint_issues_a_distinct_nonce_and_record() {
    let nonces = nonces();
    let first = nonces.mint(ISSUER, JKT, 1000).expect("first");
    let second = nonces.mint(ISSUER, JKT, 1000).expect("second");

    assert_ne!(first.nonce, second.nonce);
    assert_ne!(first.record, second.record);
}

#[test]
fn check_refuses_a_nonce_issued_to_another_key() {
    let nonces = nonces();
    let issued = nonces.mint(ISSUER, "other-thumbprint", 1000).expect("mint");

    let err = nonces
        .check(issued.nonce.as_str(), ISSUER, JKT, 1000, skew())
        .expect_err("other key refused");

    assert!(matches!(err, NonceError::KeyMismatch), "got {err:?}");
}

#[test]
fn check_refuses_a_nonce_another_resource_issued() {
    let nonces = nonces();
    let issued = nonces.mint("https://other.example", JKT, 1000).expect("mint");

    let err = nonces
        .check(issued.nonce.as_str(), ISSUER, JKT, 1000, skew())
        .expect_err("other issuer refused");

    assert!(matches!(err, NonceError::WrongIssuer), "got {err:?}");
}

#[test]
fn check_accepts_a_nonce_up_to_the_skew_past_its_expiry() {
    let nonces = nonces();
    let issued = nonces.mint(ISSUER, JKT, 1000).expect("mint");

    assert!(nonces.check(issued.nonce.as_str(), ISSUER, JKT, 1090, skew()).is_ok());
    let err = nonces
        .check(issued.nonce.as_str(), ISSUER, JKT, 1091, skew())
        .expect_err("expired refused");
    assert!(matches!(err, NonceError::Expired), "got {err:?}");
}

#[test]
fn check_refuses_a_nonce_issued_further_ahead_than_the_skew() {
    let nonces = nonces();
    let issued = nonces.mint(ISSUER, JKT, 1031).expect("mint");

    let err = nonces
        .check(issued.nonce.as_str(), ISSUER, JKT, 1000, skew())
        .expect_err("future nonce refused");

    assert!(matches!(err, NonceError::NotYetValid), "got {err:?}");
}

#[test]
fn check_refuses_a_nonce_whose_window_outlives_the_longest_lifetime() {
    let nonces = nonces();
    let long = sign_claims(
        &nonces.key,
        TYP_SERVER_NONCE,
        "nonce-k1",
        &claims(1000, 1000 + i64::from(MAX_SERVER_NONCE_TTL_SECS) + 1),
    );

    let err = nonces
        .check(&long, ISSUER, JKT, 1000, skew())
        .expect_err("over-long window refused");

    assert!(matches!(err, NonceError::InvalidWindow), "got {err:?}");
}

#[test]
fn check_refuses_a_nonce_whose_window_ends_before_it_starts() {
    let nonces = nonces();
    let backwards = sign_claims(&nonces.key, TYP_SERVER_NONCE, "nonce-k1", &claims(1000, 1000));

    let err = nonces
        .check(&backwards, ISSUER, JKT, 1000, skew())
        .expect_err("empty window refused");

    assert!(matches!(err, NonceError::InvalidWindow), "got {err:?}");
}

#[test]
fn check_refuses_a_token_of_another_type_signed_with_the_challenge_key() {
    let nonces = nonces();
    let resource = sign_claims(&nonces.key, "aa-resource+jwt", "nonce-k1", &claims(1000, 1060));

    let err = nonces
        .check(&resource, ISSUER, JKT, 1000, skew())
        .expect_err("other typ refused");

    assert!(matches!(err, NonceError::NotANonce), "got {err:?}");
}

#[test]
fn check_refuses_a_nonce_naming_another_challenge_key() {
    let nonces = nonces();
    let foreign = sign_claims(&nonces.key, TYP_SERVER_NONCE, "nonce-k2", &claims(1000, 1060));

    let err = nonces
        .check(&foreign, ISSUER, JKT, 1000, skew())
        .expect_err("unknown kid refused");

    assert!(matches!(err, NonceError::UnknownKey), "got {err:?}");
}

#[test]
fn check_refuses_a_nonce_signed_with_another_key_under_the_same_kid() {
    let nonces = nonces();
    let impostor = challenge_key("nonce-k1");
    let forged = sign_claims(&impostor, TYP_SERVER_NONCE, "nonce-k1", &claims(1000, 1060));

    let err = nonces
        .check(&forged, ISSUER, JKT, 1000, skew())
        .expect_err("forged nonce refused");

    assert!(matches!(err, NonceError::Rejected(_)), "got {err:?}");
}

#[test]
fn check_honors_a_nonce_minted_by_another_holder_of_the_same_key() {
    let pem = challenge_key_pem();
    let issuer = nonces_with(challenge_key_from(&pem, "nonce-k1"), 60);
    let verifier = nonces_with(challenge_key_from(&pem, "nonce-k1"), 60);
    let issued = issuer.mint(ISSUER, JKT, 1000).expect("mint");

    let record = verifier
        .check(issued.nonce.as_str(), ISSUER, JKT, 1000, skew())
        .expect("other holder verifies");

    assert_eq!(record, issued.record);
}

#[test]
fn check_refuses_a_value_that_is_not_a_jws() {
    let err = nonces()
        .check("not-a-nonce", ISSUER, JKT, 1000, skew())
        .expect_err("garbage refused");

    assert!(matches!(err, NonceError::Malformed(_)), "got {err:?}");
}

#[test]
fn server_nonce_displays_its_wire_form() {
    let issued = nonces().mint(ISSUER, JKT, 1000).expect("mint");
    assert_eq!(issued.nonce.to_string(), issued.nonce.as_str());
}
