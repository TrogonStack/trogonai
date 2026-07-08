use super::*;
use crate::test_support::{jwks_with_key, p256_fixture};
use crate::time_source::SystemTimeSource;
use jsonwebtoken::EncodingKey;

fn challenge<'a>(jti: &'a str, ttl_secs: i64) -> ResourceChallenge<'a> {
    ResourceChallenge {
        iss: "ps.example",
        aud_ps: "ps.example",
        agent: "agent-1",
        agent_jkt: "abc",
        scope: "read",
        ttl_secs,
        kid: "kid-1",
        jti,
        mission: None,
    }
}

#[test]
fn mint_succeeds_with_normal_inputs() {
    // Pin a real ES256 key so this exercises the happy path (proving
    // the Encode source-wrapping path doesn't fire for valid input).
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let key = EncodingKey::from_ec_pem(pem).expect("test key");
    let minter = ChallengeMinter::new(key, Algorithm::ES256, FixedClock(1000));
    let token = minter.mint(&challenge("jti-1", 60)).expect("mint succeeds");
    assert!(token.split('.').count() == 3, "got {token}");
}

#[test]
fn mint_returns_ttl_overflow_at_i64_max() {
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let key = EncodingKey::from_ec_pem(pem).expect("test key");
    let minter = ChallengeMinter::new(key, Algorithm::ES256, FixedClock(i64::MAX));
    let err = minter.mint(&challenge("jti-2", 1)).unwrap_err();
    assert!(
        matches!(err, ChallengeError::TtlOverflow { iat, ttl_secs } if iat == i64::MAX && ttl_secs == 1),
        "got {err:?}"
    );
}

#[test]
fn mint_resource_jwt_one_shot_helper() {
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let key = EncodingKey::from_ec_pem(pem).expect("test key");
    let token = mint_resource_jwt(&key, Algorithm::ES256, 1234, &challenge("jti-3", 60)).expect("one-shot mint");
    assert_eq!(token.split('.').count(), 3);
}

#[test]
fn challenge_error_ttl_overflow_display_mentions_inputs() {
    let msg = format!("{}", ChallengeError::TtlOverflow { iat: 100, ttl_secs: 5 });
    assert!(msg.contains("100"));
    assert!(msg.contains('5'));
}

struct FixedClock(i64);
impl TimeSource for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

fn base_resource_claims() -> serde_json::Value {
    serde_json::json!({
        "iss": "resource.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "aauth:asst@agent.example",
        "agent_jkt": "signing-jkt",
        "scope": "read",
    })
}

fn challenge_ctx() -> ResourceChallengeContext<'static> {
    ResourceChallengeContext {
        requested_resource: "resource.example",
        own_agent_identifier: "aauth:asst@agent.example",
        own_signing_jkt: "signing-jkt",
    }
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_challenge_accepts_matching_context() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("resource.example", fixture.jwk.clone());
    let jwt = {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some("k1".into());
        jsonwebtoken::encode(&header, &base_resource_claims(), &fixture.signing).expect("encode")
    };
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let verified = verify_resource_challenge(&verifier, &jwt, "ps.example", &challenge_ctx(), 1_000_000)
        .await
        .expect("should verify");
    assert_eq!(verified.claims.agent, "aauth:asst@agent.example");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_challenge_rejects_issuer_mismatch() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("other-resource.example", fixture.jwk.clone());
    let mut claims = base_resource_claims();
    claims["iss"] = serde_json::Value::String("other-resource.example".into());
    let jwt = {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some("k1".into());
        jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode")
    };
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let err = verify_resource_challenge(&verifier, &jwt, "ps.example", &challenge_ctx(), 1_000_000)
        .await
        .unwrap_err();
    assert!(matches!(err, ResourceChallengeError::IssuerMismatch { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_challenge_rejects_agent_mismatch() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("resource.example", fixture.jwk.clone());
    let mut claims = base_resource_claims();
    claims["agent"] = serde_json::Value::String("aauth:someone-else@agent.example".into());
    let jwt = {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some("k1".into());
        jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode")
    };
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let err = verify_resource_challenge(&verifier, &jwt, "ps.example", &challenge_ctx(), 1_000_000)
        .await
        .unwrap_err();
    assert!(matches!(err, ResourceChallengeError::AgentMismatch { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_challenge_rejects_signing_key_mismatch() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("resource.example", fixture.jwk.clone());
    let mut claims = base_resource_claims();
    claims["agent_jkt"] = serde_json::Value::String("different-jkt".into());
    let jwt = {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some("k1".into());
        jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode")
    };
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let err = verify_resource_challenge(&verifier, &jwt, "ps.example", &challenge_ctx(), 1_000_000)
        .await
        .unwrap_err();
    assert!(matches!(err, ResourceChallengeError::SigningKeyMismatch { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_challenge_rejects_expired_token_at_context_check() {
    let fixture = p256_fixture("k1");
    let jwks = jwks_with_key("resource.example", fixture.jwk.clone());
    let mut claims = base_resource_claims();
    // exp far in the future so `verify_resource`'s own freshness check
    // (rule 2/#assert_freshness) passes; the caller-supplied `now` for the
    // rule-6 re-check is set beyond `exp` to exercise this method's own
    // expiry check independent of the token decode step.
    claims["exp"] = serde_json::json!(2_000_000);
    let jwt = {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some("k1".into());
        jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode")
    };
    // `verify_resource`'s own freshness check (rule 2) runs against the
    // verifier's clock, which must therefore also sit before `exp` -- only
    // the explicit `now` passed to `verify_resource_challenge` (rule 6) is
    // set beyond `exp`, isolating this method's own expiry re-check.
    let verifier = TokenVerifier::new(jwks, FixedClock(1_500_000));
    let err = verify_resource_challenge(&verifier, &jwt, "ps.example", &challenge_ctx(), 3_000_000)
        .await
        .unwrap_err();
    assert!(matches!(err, ResourceChallengeError::Expired));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_challenge_propagates_token_error() {
    let jwks = crate::jwks::StaticJwks::new();
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let err = verify_resource_challenge(
        &verifier,
        "not.a.jwt.actually",
        "ps.example",
        &challenge_ctx(),
        1_000_000,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ResourceChallengeError::Token(_)));
}

#[test]
fn resource_challenge_error_display_messages_are_distinct() {
    let cases = [
        format!(
            "{}",
            ResourceChallengeError::IssuerMismatch {
                expected: "a".into(),
                actual: "b".into(),
            }
        ),
        format!(
            "{}",
            ResourceChallengeError::AgentMismatch {
                expected: "a".into(),
                actual: "b".into(),
            }
        ),
        format!(
            "{}",
            ResourceChallengeError::SigningKeyMismatch {
                expected: "a".into(),
                actual: "b".into(),
            }
        ),
        format!("{}", ResourceChallengeError::Expired),
    ];
    for i in 0..cases.len() {
        for j in (i + 1)..cases.len() {
            assert_ne!(cases[i], cases[j]);
        }
    }
}
