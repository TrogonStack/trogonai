use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use tower::ServiceExt;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TimeSource, TokenVerifier};
use trogon_identity_types::aauth::person_server::TokenRequest;
use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims, TYP_AGENT, TYP_RESOURCE};

use super::*;
use crate::decision::{DecisionRequest, PolicyDecision, PolicyEngine};
use crate::interaction::NoopInteractionChannel;
use crate::store::InMemoryStore;

fn now_unix() -> i64 {
    SystemTimeSource.now()
}

struct KeyFixture {
    encoding_key: EncodingKey,
    jwk: serde_json::Value,
    jwk_set: jsonwebtoken::jwk::JwkSet,
}

fn key_fixture(kid: &str) -> KeyFixture {
    let signing_key = SigningKey::random(&mut OsRng);
    let pkcs8_pem = signing_key.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
    let encoding_key = EncodingKey::from_ec_pem(pkcs8_pem.as_bytes()).unwrap();
    let point = signing_key.verifying_key().to_encoded_point(false);
    let x = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, point.x().unwrap());
    let y = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, point.y().unwrap());
    let jwk = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid});
    let jwk_set: jsonwebtoken::jwk::JwkSet = serde_json::from_value(serde_json::json!({"keys": [jwk]})).unwrap();
    KeyFixture {
        encoding_key,
        jwk,
        jwk_set,
    }
}

fn mint_agent_jwt(fixture: &KeyFixture, iss: &str, sub: &str, kid: &str) -> String {
    let now = now_unix();
    let claims = AgentClaims {
        iss: iss.to_string(),
        sub: sub.to_string(),
        jti: "agent-jti".to_string(),
        iat: now - 5,
        exp: now + 600,
        dwk: "aauth-agent.json".to_string(),
        cnf: Cnf {
            jwk: fixture.jwk.clone(),
        },
        ps: None,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(kid.to_string());
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

fn mint_resource_jwt(fixture: &KeyFixture, iss: &str, aud: &str, agent: &str, agent_jkt: &str, kid: &str) -> String {
    let now = now_unix();
    let claims = ResourceClaims {
        iss: iss.to_string(),
        aud: aud.to_string(),
        jti: "resource-jti".to_string(),
        iat: now - 5,
        exp: now + 600,
        dwk: "aauth-resource.json".to_string(),
        agent: agent.to_string(),
        agent_jkt: agent_jkt.to_string(),
        scope: "calendar.readwrite".to_string(),
        mission: None,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some(kid.to_string());
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

fn jkt_of(jwk: &serde_json::Value) -> String {
    trogon_aauth_verify::jwk_thumbprint(jwk).unwrap()
}

#[derive(Debug, thiserror::Error)]
#[error("stub policy error")]
struct StubPolicyError;

struct ScriptedPolicy {
    decisions: StdMutex<Vec<PolicyDecision>>,
}

impl ScriptedPolicy {
    fn new(decisions: Vec<PolicyDecision>) -> Self {
        let mut decisions = decisions;
        decisions.reverse();
        Self {
            decisions: StdMutex::new(decisions),
        }
    }
}

#[async_trait]
impl PolicyEngine for ScriptedPolicy {
    type Error = StubPolicyError;

    async fn decide(&self, _request: DecisionRequest<'_>) -> Result<PolicyDecision, Self::Error> {
        let mut decisions = self.decisions.lock().unwrap();
        decisions.pop().ok_or(StubPolicyError)
    }
}

type TestServer = PersonServer<StaticJwks, SystemTimeSource, ScriptedPolicy, NoopInteractionChannel, InMemoryStore>;

fn build_router(decisions: Vec<PolicyDecision>, jwks: StaticJwks) -> Router {
    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_pem = ps_signing.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
    let ps_encoding_key = EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap();

    let server: TestServer = PersonServer::new(
        TokenVerifier::new(jwks, SystemTimeSource),
        SystemTimeSource,
        ScriptedPolicy::new(decisions),
        NoopInteractionChannel,
        InMemoryStore::new(),
        ps_encoding_key,
        Algorithm::ES256,
        "ps-kid",
        "https://ps.example",
    );
    router(Arc::new(server))
}

fn agent_and_resource() -> (String, String) {
    let agent_fixture = key_fixture("agent-kid");
    let resource_fixture = key_fixture("resource-kid");
    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
    );
    (agent_jwt, resource_jwt)
}

fn jwks_for(agent_jwt: &str, resource_jwt: &str) -> StaticJwks {
    // Rebuild JWKS from the same fixture material by re-deriving issuers;
    // tests call this immediately after `agent_and_resource`, so instead we
    // thread jwk sets through explicitly in each test rather than
    // reconstructing them from tokens (JWTs don't carry the full JWK).
    let _ = (agent_jwt, resource_jwt);
    StaticJwks::new()
}

#[tokio::test]
async fn token_endpoint_grants_returns_200_with_auth_token() {
    let agent_fixture = key_fixture("agent-kid");
    let resource_fixture = key_fixture("resource-kid");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
    );

    let app = build_router(
        vec![PolicyDecision::Grant {
            scope: "calendar.readwrite".to_string(),
        }],
        jwks,
    );

    let body = serde_json::to_vec(&TokenRequest::new(resource_jwt)).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header("content-type", "application/json")
        .header("signature-key", format!("sig=jwt;jwt=\"{agent_jwt}\""))
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["auth_token"].is_string());
}

#[tokio::test]
async fn token_endpoint_without_signature_key_returns_400() {
    let (_agent_jwt, resource_jwt) = agent_and_resource();
    let app = build_router(vec![], jwks_for("", ""));

    let body = serde_json::to_vec(&TokenRequest::new(resource_jwt)).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "invalid_agent_token");
}

#[tokio::test]
async fn token_endpoint_needs_interaction_returns_202_with_location() {
    let agent_fixture = key_fixture("agent-kid");
    let resource_fixture = key_fixture("resource-kid");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
    );

    let app = build_router(vec![PolicyDecision::NeedsInteraction], jwks);

    let body = serde_json::to_vec(&TokenRequest::new(resource_jwt)).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header("content-type", "application/json")
        .header("signature-key", format!("sig=jwt;jwt=\"{agent_jwt}\""))
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(response.headers().get(axum::http::header::LOCATION).is_some());
}

#[tokio::test]
async fn poll_endpoint_returns_404_style_error_for_unknown_id() {
    let jwks = StaticJwks::new();
    let app = build_router(vec![], jwks);

    let request = Request::builder()
        .method("GET")
        .uri("/token/does-not-exist")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn login_endpoint_parses_query_string() {
    let jwks = StaticJwks::new();
    let app = build_router(vec![], jwks);

    let request = Request::builder()
        .method("GET")
        .uri("/login?ps=https%3A%2F%2Fps.example&tenant=corp")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["ps"], "https://ps.example");
}

#[tokio::test]
async fn login_endpoint_without_ps_returns_400() {
    let jwks = StaticJwks::new();
    let app = build_router(vec![], jwks);

    let request = Request::builder()
        .method("GET")
        .uri("/login?tenant=corp")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audit_endpoint_requires_known_mission() {
    let jwks = StaticJwks::new();
    let app = build_router(vec![], jwks);

    let body = serde_json::to_vec(&serde_json::json!({
        "mission": {"approver": "https://ps.example", "s256": "unknown-mission"},
        "action": "WebSearch"
    }))
    .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/audit")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
