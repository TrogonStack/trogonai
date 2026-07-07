use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::Algorithm;
use tower::ServiceExt;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TokenVerifier};
use trogon_identity_types::aauth::federation::AsTokenRequest;

use super::*;
use crate::policy::AlwaysIssuePolicy;
use crate::server::{AccessServer, AccessServerConfig, FederationMode};
use crate::test_support::{jwk_set, key_fixture, mint_agent_jwt, mint_resource_jwt, now_unix};
use crate::trust::{TrustBasisRecord, TrustRegistry};

const AS_ISS: &str = "https://as.example";
const PS_ISS: &str = "https://ps.example";
const AP_ISS: &str = "https://ap.example";
const RESOURCE_ISS: &str = "https://resource.example";

fn build_server_and_request(
    scope: &str,
    now: i64,
) -> (
    Arc<AccessServer<StaticJwks, SystemTimeSource, AlwaysIssuePolicy>>,
    AsTokenRequest,
) {
    let as_key = key_fixture("as-kid-h");
    let ap_key = key_fixture("ap-kid-h");
    let resource_key = key_fixture("resource-kid-h");
    let agent_key = key_fixture("agent-key-h");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-h",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-h",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:asst@agent.example",
        &agent_jkt,
        scope,
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let cfg = AccessServerConfig {
        iss: AS_ISS.to_string(),
        signing_key: as_key.encoding_key,
        alg: Algorithm::ES256,
        kid: "as-kid-h".to_string(),
        auth_token_ttl: crate::mint::AuthTokenTtl::new(300).unwrap(),
        mode: FederationMode::Federated,
    };
    let server = Arc::new(AccessServer::new(
        verifier,
        SystemTimeSource,
        trust,
        AlwaysIssuePolicy,
        cfg,
    ));

    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };
    (server, req)
}

#[tokio::test]
async fn token_endpoint_returns_200_with_auth_token_for_trusted_ps() {
    let now = now_unix();
    let (server, req) = build_server_and_request("documents:read", now);
    let app = router(server).layer(axum::Extension(PsIdentity {
        iss: PS_ISS.to_string(),
    }));

    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&req).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!value["auth_token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn token_endpoint_returns_error_for_untrusted_ps() {
    let now = now_unix();
    let (server, req) = build_server_and_request("documents:read", now);
    let app = router(server).layer(axum::Extension(PsIdentity {
        iss: "https://unknown-ps.example".to_string(),
    }));

    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&req).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn resume_endpoint_returns_not_found_for_unknown_pending_id() {
    let now = now_unix();
    let (server, _req) = build_server_and_request("documents:read", now);
    let app = router(server).layer(axum::Extension(PsIdentity {
        iss: PS_ISS.to_string(),
    }));

    let submission = trogon_identity_types::aauth::federation::ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    let request = Request::builder()
        .method("POST")
        .uri("/token/pending/does-not-exist")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&submission).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
