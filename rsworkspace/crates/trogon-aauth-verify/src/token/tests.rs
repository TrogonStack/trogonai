use super::*;
use crate::jwks::StaticJwks;
use crate::test_support::{ed25519_fixture, jwks_with_key, p256_fixture, p384_fixture};
use crate::time_source::SystemTimeSource;
use std::sync::Arc;

fn freshness_clock(value: Arc<std::sync::atomic::AtomicI64>) -> impl TimeSource {
    struct C(Arc<std::sync::atomic::AtomicI64>);
    impl TimeSource for C {
        fn now(&self) -> i64 {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    C(value)
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_accepts_es384_jwk() {
    let fixture = p384_fixture();
    let jwks = jwks_with_key("iss.example", fixture.jwk);
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("p384-k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    v.verify_resource(&jwt, "ps.example")
        .await
        .expect("ES384 resource token verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_accepts_eddsa_jwk() {
    let fixture = ed25519_fixture("ed-k1");
    let jwks = jwks_with_key("iss.example", fixture.jwk);
    let mut header = jsonwebtoken::Header::new(Algorithm::EdDSA);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("ed-k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.encoding).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    v.verify_resource(&jwt, "ps.example")
        .await
        .expect("EdDSA resource token verifies");
}

#[tokio::test(flavor = "current_thread")]
async fn pick_jwk_rejects_incompatible_curve_in_set() {
    let es384 = p384_fixture();
    let p256 = p256_fixture("p256-only");
    let mut header = jsonwebtoken::Header::new(Algorithm::ES384);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("p384-k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &es384.signing).expect("encode");
    let jwks = jwks_with_key("iss.example", p256.jwk);
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let err = v.verify_resource(&jwt, "ps.example").await.unwrap_err();
    assert!(matches!(err, TokenError::NoCompatibleJwk), "got {err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn assert_freshness_uses_supplied_clock() {
    let now = Arc::new(std::sync::atomic::AtomicI64::new(1000));
    let v = TokenVerifier::new(StaticJwks::new(), freshness_clock(now.clone())).with_leeway(0);

    v.assert_freshness(900, 1100).expect("freshness inside window");

    now.store(1201, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(v.assert_freshness(900, 1100), Err(TokenError::Expired)));

    now.store(500, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(v.assert_freshness(900, 1100), Err(TokenError::NotYetValid)));

    let _system: TokenVerifier<StaticJwks, SystemTimeSource> = TokenVerifier::new(StaticJwks::new(), SystemTimeSource);
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_distinguishes_audience_mismatch_from_signature() {
    // Regression: jsonwebtoken collapses InvalidAudience into the same
    // error type as bad signatures. The verifier must surface it as the
    // typed AudienceMismatch variant so operators can tell the two
    // failure modes apart.
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(pem).expect("signing key");
    let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "kid": "k1",
        "alg": "ES256",
        "use": "sig",
        "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
        "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY"
    }))
    .expect("jwk");
    let jwks = StaticJwks::new().with("iss.example", jsonwebtoken::jwk::JwkSet { keys: vec![jwk] });

    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let err = v.verify_resource(&jwt, "WRONG-AUD").await.unwrap_err();
    assert!(
        matches!(&err, TokenError::AudienceMismatch { expected, .. } if expected == "WRONG-AUD"),
        "got {err:?}"
    );
}

const P256_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";

fn p256_jwk_for_test() -> jsonwebtoken::jwk::Jwk {
    serde_json::from_value(serde_json::json!({
        "kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
        "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
        "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY"
    }))
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn verify_auth_happy_path() {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "sub": "person-1",
        "aud": "resource.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let verified = v.verify_auth(&jwt, "resource.example").await.expect("verify_auth");
    assert_eq!(verified.claims.iss, "iss.example");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_happy_path() {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let verified = v.verify_resource(&jwt, "ps.example").await.expect("verify_resource");
    assert_eq!(verified.claims.iss, "iss.example");
}

#[tokio::test(flavor = "current_thread")]
async fn parse_typ_rejects_wrong_typ() {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some("aa-other+jwt".into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({"iss": "iss.example"});
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(StaticJwks::new(), SystemTimeSource);
    let err = v.verify_agent(&jwt).await.unwrap_err();
    assert!(matches!(
        err,
        TokenError::WrongTyp {
            expected: TYP_AGENT,
            ..
        }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn parse_typ_rejects_unsupported_alg() {
    let mut header = jsonwebtoken::Header::new(Algorithm::HS256);
    header.typ = Some(TYP_AGENT.into());
    let claims = serde_json::json!({"iss": "iss.example"});
    let jwt = jsonwebtoken::encode(&header, &claims, &jsonwebtoken::EncodingKey::from_secret(b"x")).expect("encode");
    let v = TokenVerifier::new(StaticJwks::new(), SystemTimeSource);
    let err = v.verify_agent(&jwt).await.unwrap_err();
    assert!(matches!(err, TokenError::UnsupportedAlg(Algorithm::HS256)));
}

#[tokio::test(flavor = "current_thread")]
async fn iss_of_rejects_malformed_jwt() {
    // Two-segment JWT — missing payload.
    let v = TokenVerifier::new(StaticJwks::new(), SystemTimeSource);
    let err = v.verify_agent("not.a.jwt.actually").await.unwrap_err();
    // parse_typ tries decode_header first; an undecodable header returns BadHeader.
    assert!(matches!(err, TokenError::BadHeader));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_agent_happy_path() {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "sub": "agent-1",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-agent",
        "cnf": { "jwk": {
            "kty": "EC", "crv": "P-256",
            "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
            "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY"
        }},
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let verified = v.verify_agent(&jwt).await.expect("verify_agent");
    assert_eq!(verified.claims.iss, "iss.example");
    assert!(!verified.jkt.is_empty());
}

async fn verify_agent_with_cnf_jwk(cnf_jwk: serde_json::Value) -> TokenError {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "sub": "agent-1",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-agent",
        "cnf": { "jwk": cnf_jwk },
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    v.verify_agent(&jwt).await.unwrap_err()
}

#[tokio::test(flavor = "current_thread")]
async fn verify_agent_maps_missing_field_to_invalid_claim() {
    // EC kty present but missing required `x` — triggers MissingField arm.
    let err = verify_agent_with_cnf_jwk(serde_json::json!({"kty": "EC", "crv": "P-256", "y": "zzz"})).await;
    assert!(matches!(err, TokenError::InvalidClaim("x")));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_agent_maps_unsupported_kty_to_invalid_claim() {
    let err = verify_agent_with_cnf_jwk(serde_json::json!({"kty": "BOGUS"})).await;
    assert!(matches!(err, TokenError::InvalidClaim("cnf.jwk.kty")));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_agent_maps_missing_cnf_to_invalid_claim() {
    // Drives the JktError mapping arms in verify_agent: a present-but-malformed
    // cnf.jwk (missing kty) returns InvalidClaim, not MissingClaim.
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "sub": "agent-1",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-agent",
        "cnf": { "jwk": { "crv": "P-256" } }, // missing kty
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let err = v.verify_agent(&jwt).await.unwrap_err();
    assert!(matches!(err, TokenError::InvalidClaim(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn pick_jwk_falls_back_to_sole_compatible_key_when_kid_absent() {
    // Drives the single-key fallback in pick_jwk: JWT header carries no kid
    // and the JwkSet has exactly one compatible key.
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    // No `kid` on the header forces pick_jwk into the single-key branch.
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    v.verify_resource(&jwt, "ps.example")
        .await
        .expect("single-key fallback");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_resource_signature_path_when_jwt_corrupted() {
    // Crafted JWT with a valid header + payload but a garbage signature drives
    // the non-InvalidAudience Signature arm in decode_with_jwks.
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let jwks = StaticJwks::new().with(
        "iss.example",
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("k1".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "jti": "j1",
        "iat": 1000,
        "exp": 9999999999_i64,
        "dwk": "aa-resource",
        "agent": "agent-1",
        "agent_jkt": "abc",
        "scope": "read",
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
    // Truncate to invalidate the signature segment.
    let bad = format!("{}AA", &jwt[..jwt.len() - 4]);
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let err = v.verify_resource(&bad, "ps.example").await.unwrap_err();
    assert!(matches!(err, TokenError::Signature(_)));
}

#[test]
fn token_error_display_messages_are_distinct() {
    let cases = [
        format!("{}", TokenError::BadHeader),
        format!(
            "{}",
            TokenError::WrongTyp {
                expected: TYP_AGENT,
                actual: Some("x".into())
            }
        ),
        format!("{}", TokenError::UnsupportedAlg(Algorithm::HS256)),
        format!("{}", TokenError::NoCompatibleJwk),
        format!("{}", TokenError::Expired),
        format!("{}", TokenError::NotYetValid),
        format!(
            "{}",
            TokenError::AudienceMismatch {
                expected: "a".into(),
                actual: None
            }
        ),
        format!("{}", TokenError::MissingClaim("c")),
        format!("{}", TokenError::InvalidClaim("c")),
    ];
    for window in cases.windows(2) {
        assert_ne!(window[0], window[1]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn verify_agent_returns_no_compatible_jwk_when_set_is_empty() {
    // A typed Err variant per failure mode lets the gateway distinguish
    // "issuer is known but doesn't publish a matching key" (operator
    // misconfig / pending key rotation) from a generic signature failure.
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    let key = jsonwebtoken::EncodingKey::from_ec_pem(pem).expect("test key");
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some("missing-kid".into());
    let claims = serde_json::json!({
        "iss": "iss.example",
        "aud": "ps.example",
        "iat": 1000,
        "exp": 2000,
    });
    let jwt = jsonwebtoken::encode(&header, &claims, &key).expect("encode");
    let jwks = StaticJwks::new().with("iss.example", jsonwebtoken::jwk::JwkSet { keys: vec![] });
    let v = TokenVerifier::new(jwks, SystemTimeSource);
    let err = v.verify_agent(&jwt).await.unwrap_err();
    assert!(matches!(err, TokenError::NoCompatibleJwk), "got {err:?}");
}
