use super::*;
use crate::jwks::StaticJwks;
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
