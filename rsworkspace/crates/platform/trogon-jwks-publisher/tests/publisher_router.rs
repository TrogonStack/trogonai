//! Drives `trogon_jwks_publisher::publisher::router` through `tower::oneshot`
//! with no live sockets, per the a2a-nats-http testing pattern.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use tower::ServiceExt as _;
use trogon_identity_types::aauth::DWK_AGENT;
use trogon_jwks_publisher::publisher::{CacheMaxAge, JwksPublisherConfigBuilder, router};

fn sample_jwk_set() -> JwkSet {
    JwkSet {
        keys: vec![Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("k1".to_string()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
                key_type: EllipticCurveKeyType::EC,
                curve: EllipticCurve::P256,
                x: "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84".to_string(),
                y: "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY".to_string(),
            }),
        }],
    }
}

#[tokio::test(flavor = "current_thread")]
async fn well_known_agent_dwk_returns_configured_jwk_set() {
    let jwk_set = sample_jwk_set();
    let config = JwksPublisherConfigBuilder::new(CacheMaxAge::new(300))
        .with_jwk_set(DWK_AGENT, jwk_set.clone())
        .expect("known dwk registers")
        .build();
    let app = router(config);

    let request = Request::builder()
        .uri("/.well-known/aauth-agent.json")
        .body(Body::empty())
        .expect("build request");
    let response = app.oneshot(request).await.expect("router responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("max-age=300")
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/jwk-set+json")
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("body is json");
    let expected_json = serde_json::to_value(&jwk_set).expect("serialize expected");
    assert_eq!(body_json, expected_json);
}

#[tokio::test(flavor = "current_thread")]
async fn unregistered_dwk_returns_404() {
    let config = JwksPublisherConfigBuilder::new(CacheMaxAge::new(300))
        .with_jwk_set(DWK_AGENT, sample_jwk_set())
        .expect("known dwk registers")
        .build();
    let app = router(config);

    let request = Request::builder()
        .uri("/.well-known/nonsense.json")
        .body(Body::empty())
        .expect("build request");
    let response = app.oneshot(request).await.expect("router responds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
async fn known_but_unregistered_dwk_returns_404() {
    let config = JwksPublisherConfigBuilder::new(CacheMaxAge::new(300))
        .with_jwk_set(DWK_AGENT, sample_jwk_set())
        .expect("known dwk registers")
        .build();
    let app = router(config);

    let request = Request::builder()
        .uri("/.well-known/aauth-resource.json")
        .body(Body::empty())
        .expect("build request");
    let response = app.oneshot(request).await.expect("router responds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
