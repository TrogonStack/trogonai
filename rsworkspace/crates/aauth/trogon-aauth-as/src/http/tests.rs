use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::Algorithm;
use tower::ServiceExt;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TokenVerifier};
use trogon_identity_types::aauth::federation::AsTokenRequest;

use super::*;
use crate::policy::{AlwaysIssuePolicy, Decision, DenialReason, GrantedScope, OrganizationPolicy, RequiredClaims};
use crate::request::AsTokenContext;
use crate::server::{AccessServer, AccessServerConfig, FederationMode};
use crate::test_support::{jwk_set, key_fixture, mint_agent_jwt, mint_resource_jwt, now_unix};
use crate::trust::{TrustBasisRecord, TrustRegistry, TrustedIssuer};

const AS_ISS: &str = "https://as.example";
const PS_ISS: &str = "https://ps.example";
const AP_ISS: &str = "https://ap.example";
const RESOURCE_ISS: &str = "https://resource.example";

/// Keys off `resource_claims.scope` so a single policy can drive issue,
/// deny, and claims-required transitions through the HTTP layer without a
/// mock crate, mirroring `server::tests::ScopeSwitchedPolicy`.
struct ScopeSwitchedPolicy;

impl OrganizationPolicy for ScopeSwitchedPolicy {
    fn decide(&self, ctx: &AsTokenContext<'_>, _trust: &TrustedIssuer) -> Decision {
        match ctx.resource_claims.scope.as_str() {
            "deny-me" => Decision::Deny {
                reason: DenialReason::new("org policy denies this scope"),
            },
            "claims-please" => Decision::RequireClaims {
                required: RequiredClaims::new(["email"]).unwrap(),
            },
            scope => Decision::Issue {
                scope: GrantedScope::new(scope),
            },
        }
    }

    fn decide_with_claims(
        &self,
        ctx: &AsTokenContext<'_>,
        _trust: &TrustedIssuer,
        _claims: &trogon_identity_types::aauth::federation::ClaimsSubmission,
    ) -> Decision {
        Decision::Issue {
            scope: GrantedScope::new(ctx.resource_claims.scope.clone()),
        }
    }
}

fn build_scope_switched_server_and_request(
    scope: &str,
    now: i64,
) -> (
    Arc<AccessServer<StaticJwks, SystemTimeSource, ScopeSwitchedPolicy>>,
    AsTokenRequest,
) {
    let as_key = key_fixture("as-kid-sw");
    let ap_key = key_fixture("ap-kid-sw");
    let resource_key = key_fixture("resource-kid-sw");
    let agent_key = key_fixture("agent-key-sw");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-sw",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-sw",
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
        kid: "as-kid-sw".to_string(),
        auth_token_ttl: crate::mint::AuthTokenTtl::new(300).unwrap(),
        mode: FederationMode::Federated,
    };
    let server = Arc::new(AccessServer::new(
        verifier,
        SystemTimeSource,
        trust,
        ScopeSwitchedPolicy,
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

#[tokio::test]
async fn token_endpoint_returns_403_denied_with_reason_in_body() {
    let now = now_unix();
    let (server, req) = build_scope_switched_server_and_request("deny-me", now);
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
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"], "denied");
    assert_eq!(value["detail"], "org policy denies this scope");
}

#[tokio::test]
async fn token_endpoint_returns_202_with_location_and_requirement_header() {
    let now = now_unix();
    let (server, req) = build_scope_switched_server_and_request("claims-please", now);
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
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let headers = response.headers().clone();
    let location = headers.get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
    assert!(location.starts_with("/token/pending/"));
    assert_eq!(
        headers.get("aauth-requirement").unwrap().to_str().unwrap(),
        "requirement=claims"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "pending");
    assert_eq!(value["required_claims"], serde_json::json!(["email"]));
}

#[tokio::test]
async fn resume_endpoint_with_ps_identity_resumes_and_issues() {
    let now = now_unix();
    let (server, req) = build_scope_switched_server_and_request("claims-please", now);
    let app = router(server.clone()).layer(axum::Extension(PsIdentity {
        iss: PS_ISS.to_string(),
    }));

    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&req).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let location = response
        .headers()
        .get(axum::http::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let submission = trogon_identity_types::aauth::federation::ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    let resume_request = Request::builder()
        .method("POST")
        .uri(location)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&submission).unwrap()))
        .unwrap();
    let resume_response = app.oneshot(resume_request).await.unwrap();
    assert_eq!(resume_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resume_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!value["auth_token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn token_endpoint_returns_400_invalid_resource_token_for_bad_audience() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-h2");
    let ap_key = key_fixture("ap-kid-h2");
    let resource_key = key_fixture("resource-kid-h2");
    let agent_key = key_fixture("agent-key-h2");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-h2",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    // aud names a different AS than the one verifying this request.
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-h2",
        RESOURCE_ISS,
        "https://other-as.example",
        "aauth:asst@agent.example",
        &agent_jkt,
        "documents:read",
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
        kid: "as-kid-h2".to_string(),
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"], "invalid_resource_token");
}

#[tokio::test]
async fn token_endpoint_returns_400_invalid_agent_token_for_subagent_signed_request() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-h3");
    let ap_key = key_fixture("ap-kid-h3");
    let resource_key = key_fixture("resource-kid-h3");
    let agent_key = key_fixture("agent-key-h3");

    // agent_token itself carries parent_agent -- must not directly request
    // authorization (single-level depth).
    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-h3",
        AP_ISS,
        "aauth:sub@agent.example",
        &agent_key.jwk_json,
        Some("aauth:parent@agent.example"),
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-h3",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:sub@agent.example",
        &agent_jkt,
        "documents:read",
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
        kid: "as-kid-h3".to_string(),
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"], "invalid_agent_token");
}

#[tokio::test]
async fn token_endpoint_returns_400_invalid_request_for_untrusted_upstream_issuer() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-h4");
    let ap_key = key_fixture("ap-kid-h4");
    let resource_key = key_fixture("resource-kid-h4");
    let agent_key = key_fixture("agent-key-h4");
    let untrusted_upstream_key = key_fixture("untrusted-upstream-kid-h4");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-h4",
        AP_ISS,
        "aauth:intermediary@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-h4",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:intermediary@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );
    let upstream_token = crate::test_support::mint_auth_jwt_raw(
        &untrusted_upstream_key,
        "untrusted-upstream-kid-h4",
        "https://untrusted-as.example",
        AP_ISS,
        "user:alice",
        "aauth:root@agent.example",
        "root-jkt",
        &serde_json::json!({"kty": "EC"}),
        "documents:read",
        None,
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]))
        .with("https://untrusted-as.example", jwk_set(&[&untrusted_upstream_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let cfg = AccessServerConfig {
        iss: AS_ISS.to_string(),
        signing_key: as_key.encoding_key,
        alg: Algorithm::ES256,
        kid: "as-kid-h4".to_string(),
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
        upstream_token: Some(upstream_token),
    };
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"], "invalid_request");
}

#[test]
fn verification_status_maps_untrusted_ps_to_403_invalid_request() {
    let err = RequestVerificationError::UntrustedPs(crate::trust::TrustRegistryError::UnknownIssuer(
        "https://unknown-ps.example".to_string(),
    ));
    let (status, code) = verification_status(&err);
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(code, "invalid_request");
}

#[test]
fn error_response_maps_mint_error_to_500_server_error() {
    let err = AccessServerError::Mint(crate::error::MintError::TtlOverflow {
        iat: i64::MAX,
        ttl_secs: 10,
    });
    let response = error_response(&err);
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn error_response_maps_unknown_pending_request_to_404_invalid_request() {
    let err = AccessServerError::UnknownPendingRequest("does-not-exist".to_string());
    let response = error_response(&err);
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// `AccessServerError::Denied` is not constructed anywhere in this crate
/// today (`Decision::Deny` maps to `AsOutcome::Denied` instead, handled by
/// `outcome_response`) -- this exercises the `error_response` mapping arm
/// directly so the wire shape stays pinned if a future caller does produce
/// this variant.
#[test]
fn error_response_maps_denied_error_variant_to_403() {
    let err = AccessServerError::Denied {
        reason: "org policy denies this scope".to_string(),
    };
    let response = error_response(&err);
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
