use super::*;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Clone, Copy, Debug)]
enum TokenKind {
    Agent,
    Resource,
    Auth,
}

const TOKEN_KINDS: [TokenKind; 3] = [TokenKind::Agent, TokenKind::Resource, TokenKind::Auth];

async fn verify_dates(
    kind: TokenKind,
    iat: i64,
    exp: i64,
    nbf: Option<serde_json::Value>,
    now: i64,
    leeway: u64,
) -> Result<(), TokenError> {
    let fixture = p256_fixture("dates");
    let mut header = jsonwebtoken::Header::new(fixture.alg);
    header.typ = Some(
        match kind {
            TokenKind::Agent => TYP_AGENT,
            TokenKind::Resource => TYP_RESOURCE,
            TokenKind::Auth => TYP_AUTH,
        }
        .into(),
    );
    header.kid = Some("dates".into());
    let mut claims = serde_json::json!({
        "iss": "issuer.example",
        "sub": "agent-1",
        "aud": "resource.example",
        "jti": "dates-1",
        "iat": iat,
        "exp": exp,
        "dwk": match kind {
            TokenKind::Agent => "aa-agent",
            TokenKind::Resource => "aa-resource",
            TokenKind::Auth => "aa-auth",
        },
        "cnf": {"jwk": fixture.jwk_json},
        "agent": "agent-1",
        "agent_jkt": "public-thumbprint",
        "scope": "read"
    });
    if let Some(nbf) = nbf {
        claims["nbf"] = nbf;
    }
    let jwt = jsonwebtoken::encode(&header, &claims, &fixture.signing).expect("signed date fixture");
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(now));
    let verifier =
        TokenVerifier::new(jwks_with_key("issuer.example", fixture.jwk), freshness_clock(clock)).with_leeway(leeway);
    match kind {
        TokenKind::Agent => verifier.verify_agent(&jwt).await.map(|_| ()),
        TokenKind::Resource => verifier.verify_resource(&jwt, "resource.example").await.map(|_| ()),
        TokenKind::Auth => verifier.verify_auth(&jwt, "resource.example").await.map(|_| ()),
    }
}

async fn assert_future_nbf_refused(kind: TokenKind) {
    let result = verify_dates(kind, 900, 2000, Some(serde_json::json!(1001)), 1000, 0).await;
    assert!(matches!(result, Err(TokenError::NotYetValid)), "{kind:?}: {result:?}");
}

#[tokio::test]
async fn agent_refuses_future_nbf() {
    assert_future_nbf_refused(TokenKind::Agent).await;
}

#[tokio::test]
async fn resource_refuses_future_nbf() {
    assert_future_nbf_refused(TokenKind::Resource).await;
}

#[tokio::test]
async fn auth_refuses_future_nbf() {
    assert_future_nbf_refused(TokenKind::Auth).await;
}

async fn assert_malformed_nbf_refused(kind: TokenKind) {
    for nbf in [
        serde_json::Value::Null,
        serde_json::json!("1000"),
        serde_json::json!(false),
        serde_json::json!([1000]),
        serde_json::json!({"value": 1000}),
        serde_json::json!(1000.5),
        serde_json::json!(u64::MAX),
    ] {
        let result = verify_dates(kind, 900, 2000, Some(nbf), 1000, 0).await;
        match result {
            Err(TokenError::InvalidNotBefore) => {}
            Err(TokenError::Signature(error)) => {
                assert!(
                    matches!(error.kind(), jsonwebtoken::errors::ErrorKind::Json(_)),
                    "{kind:?}: {error:?}"
                );
            }
            other => panic!("{kind:?} did not refuse malformed nbf: {other:?}"),
        }
    }
}

#[tokio::test]
async fn agent_refuses_malformed_nbf() {
    assert_malformed_nbf_refused(TokenKind::Agent).await;
}

#[tokio::test]
async fn resource_refuses_malformed_nbf() {
    assert_malformed_nbf_refused(TokenKind::Resource).await;
}

#[tokio::test]
async fn auth_refuses_malformed_nbf() {
    assert_malformed_nbf_refused(TokenKind::Auth).await;
}

async fn assert_exclusive_expiry(kind: TokenKind) {
    let result = verify_dates(kind, 900, 1000, None, 1000, 0).await;
    assert!(matches!(result, Err(TokenError::Expired)), "{kind:?}: {result:?}");
}

#[tokio::test]
async fn agent_expiry_is_exclusive() {
    assert_exclusive_expiry(TokenKind::Agent).await;
}

#[tokio::test]
async fn resource_expiry_is_exclusive() {
    assert_exclusive_expiry(TokenKind::Resource).await;
}

#[tokio::test]
async fn auth_expiry_is_exclusive() {
    assert_exclusive_expiry(TokenKind::Auth).await;
}

#[tokio::test]
async fn activation_and_configured_skew_boundaries_use_the_injected_clock() {
    for kind in TOKEN_KINDS {
        for nbf in [None, Some(serde_json::json!(999)), Some(serde_json::json!(1000))] {
            verify_dates(kind, 900, 1001, nbf, 1000, 0).await.expect("active token");
        }
        verify_dates(kind, 1001, 1002, Some(serde_json::json!(1001)), 1000, 1)
            .await
            .expect("activation boundary with explicit leeway");
        verify_dates(kind, 900, 1000, None, 1000, 1)
            .await
            .expect("expiry grace with explicit leeway");
        let result = verify_dates(kind, 900, 2000, Some(serde_json::json!(1002)), 1000, 1).await;
        assert!(matches!(result, Err(TokenError::NotYetValid)), "{kind:?}: {result:?}");
        let result = verify_dates(kind, 900, 999, None, 1000, 1).await;
        assert!(matches!(result, Err(TokenError::Expired)), "{kind:?}: {result:?}");
    }
}

#[tokio::test]
async fn freshness_math_does_not_overflow_at_clock_limits() {
    for kind in TOKEN_KINDS {
        verify_dates(
            kind,
            i64::MAX - 1,
            i64::MAX,
            Some(serde_json::json!(i64::MAX - 5)),
            i64::MAX - 10,
            20,
        )
        .await
        .expect("upper clock edge with leeway");
        verify_dates(
            kind,
            i64::MIN + 1,
            i64::MIN + 15,
            Some(serde_json::json!(i64::MIN + 5)),
            i64::MIN + 10,
            20,
        )
        .await
        .expect("lower clock edge with leeway");
    }
}

#[tokio::test]
async fn unrepresentable_leeway_does_not_disable_freshness_checks() {
    for kind in TOKEN_KINDS {
        let result = verify_dates(kind, 1001, 2000, None, 1000, u64::MAX).await;
        assert!(matches!(result, Err(TokenError::NotYetValid)), "{kind:?}: {result:?}");
    }
}

#[tokio::test]
async fn invalid_signatures_are_rejected_before_malformed_nbf() {
    for kind in TOKEN_KINDS {
        let signing = p256_fixture("dates");
        let trusted = p256_fixture("dates");
        let mut header = jsonwebtoken::Header::new(signing.alg);
        header.kid = Some("dates".into());
        header.typ = Some(
            match kind {
                TokenKind::Agent => TYP_AGENT,
                TokenKind::Resource => TYP_RESOURCE,
                TokenKind::Auth => TYP_AUTH,
            }
            .into(),
        );
        let claims = serde_json::json!({"iss": "issuer.example", "aud": "resource.example", "nbf": null});
        let jwt = jsonwebtoken::encode(&header, &claims, &signing.signing).expect("signed fixture");
        let verifier = TokenVerifier::new(
            jwks_with_key("issuer.example", trusted.jwk),
            freshness_clock(Arc::new(std::sync::atomic::AtomicI64::new(1000))),
        )
        .with_leeway(0);
        let result = match kind {
            TokenKind::Agent => verifier.verify_agent(&jwt).await.map(|_| ()),
            TokenKind::Resource => verifier.verify_resource(&jwt, "resource.example").await.map(|_| ()),
            TokenKind::Auth => verifier.verify_auth(&jwt, "resource.example").await.map(|_| ()),
        };
        assert!(matches!(result, Err(TokenError::Signature(_))), "{kind:?}: {result:?}");
    }
}

fn critical_header_fixture(kind: TokenKind, crit: serde_json::Value) -> (String, StaticJwks) {
    let fixture = p256_fixture("dates");
    let mut protected = serde_json::json!({
        "alg": "ES256",
        "typ": match kind {
            TokenKind::Agent => TYP_AGENT,
            TokenKind::Resource => TYP_RESOURCE,
            TokenKind::Auth => TYP_AUTH,
        },
        "kid": "dates",
        "crit": crit,
        "extension": "required"
    });
    protected["alg"] = serde_json::to_value(fixture.alg).expect("algorithm");
    let claims = serde_json::json!({
        "iss": "issuer.example",
        "sub": "agent-1",
        "aud": "resource.example",
        "jti": "critical-1",
        "iat": 900,
        "exp": 2000,
        "dwk": match kind {
            TokenKind::Agent => "aa-agent",
            TokenKind::Resource => "aa-resource",
            TokenKind::Auth => "aa-auth",
        },
        "cnf": {"jwk": fixture.jwk_json},
        "agent": "agent-1",
        "agent_jkt": "public-thumbprint",
        "scope": "read"
    });
    let protected = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&protected).expect("protected header"));
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"));
    let signed = format!("{protected}.{payload}");
    let signature = jsonwebtoken::crypto::sign(signed.as_bytes(), &fixture.signing, fixture.alg).expect("signature");
    (
        format!("{signed}.{signature}"),
        jwks_with_key("issuer.example", fixture.jwk),
    )
}

async fn verify_encoded(kind: TokenKind, jwt: &str, jwks: StaticJwks) -> Result<(), TokenError> {
    let verifier =
        TokenVerifier::new(jwks, freshness_clock(Arc::new(std::sync::atomic::AtomicI64::new(1000)))).with_leeway(0);
    match kind {
        TokenKind::Agent => verifier.verify_agent(jwt).await.map(|_| ()),
        TokenKind::Resource => verifier.verify_resource(jwt, "resource.example").await.map(|_| ()),
        TokenKind::Auth => verifier.verify_auth(jwt, "resource.example").await.map(|_| ()),
    }
}

async fn assert_critical_header_refused(kind: TokenKind) {
    let (jwt, jwks) = critical_header_fixture(kind, serde_json::json!(["extension"]));
    let result = verify_encoded(kind, &jwt, jwks).await;
    assert!(
        matches!(result, Err(TokenError::UnsupportedCriticalHeader)),
        "{kind:?}: {result:?}"
    );
}

#[tokio::test]
async fn agent_refuses_unsupported_critical_headers() {
    assert_critical_header_refused(TokenKind::Agent).await;
}

#[tokio::test]
async fn resource_refuses_unsupported_critical_headers() {
    assert_critical_header_refused(TokenKind::Resource).await;
}

#[tokio::test]
async fn auth_refuses_unsupported_critical_headers() {
    assert_critical_header_refused(TokenKind::Auth).await;
}

#[tokio::test]
async fn empty_or_malformed_critical_headers_are_refused() {
    for kind in TOKEN_KINDS {
        for crit in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!("extension"),
            serde_json::json!(false),
            serde_json::json!([1]),
            serde_json::json!([""]),
            serde_json::json!({"name": "extension"}),
        ] {
            let (jwt, jwks) = critical_header_fixture(kind, crit);
            assert!(
                verify_encoded(kind, &jwt, jwks).await.is_err(),
                "{kind:?} accepted malformed crit"
            );
        }
    }
}

#[tokio::test]
async fn signature_failures_precede_critical_header_rejection() {
    for kind in TOKEN_KINDS {
        let (jwt, _) = critical_header_fixture(kind, serde_json::json!(["extension"]));
        let unrelated = p256_fixture("dates");
        let result = verify_encoded(kind, &jwt, jwks_with_key("issuer.example", unrelated.jwk)).await;
        assert!(matches!(result, Err(TokenError::Signature(_))), "{kind:?}: {result:?}");
    }
}

#[tokio::test]
async fn unsupported_algorithms_are_not_admitted_by_critical_headers() {
    for kind in TOKEN_KINDS {
        let (jwt, jwks) = critical_header_fixture(kind, serde_json::json!(["extension"]));
        let mut segments = jwt.split('.');
        let mut header: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(segments.next().expect("header"))
                .expect("header encoding"),
        )
        .expect("header JSON");
        header["alg"] = serde_json::json!("RS256");
        let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header JSON"));
        let token = format!(
            "{header}.{}.{}",
            segments.next().expect("payload"),
            segments.next().expect("signature")
        );
        let result = verify_encoded(kind, &token, jwks).await;
        assert!(
            matches!(result, Err(TokenError::UnsupportedAlg(Algorithm::RS256))),
            "{kind:?}: {result:?}"
        );
    }
}
