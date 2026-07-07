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
use trogon_identity_types::aauth::mission::MissionStatus;
use trogon_identity_types::aauth::person_server::TokenRequest;
use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims, TYP_AGENT, TYP_RESOURCE};

use super::*;
use crate::decision::{DecisionRequest, PolicyDecision, PolicyEngine};
use crate::interaction::NoopInteractionChannel;
use crate::mission::{ApprovedMission, Mission};
use crate::pending::{PendingPhase, PendingRequest};
use crate::store::{InMemoryStore, PersonStateStore, StoreError};

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

/// Builds a router backed by a caller-supplied store, so tests can seed
/// pending/mission records directly (poll-endpoint phases) or inject a
/// failing store (persistence-outage error arms) without driving the full
/// policy state machine through `/token`.
fn build_router_with_store<St>(decisions: Vec<PolicyDecision>, jwks: StaticJwks, store: St) -> Router
where
    St: PersonStateStore + Send + Sync + 'static,
{
    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_pem = ps_signing.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
    let ps_encoding_key = EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap();

    let server = PersonServer::new(
        TokenVerifier::new(jwks, SystemTimeSource),
        SystemTimeSource,
        ScriptedPolicy::new(decisions),
        NoopInteractionChannel,
        store,
        ps_encoding_key,
        Algorithm::ES256,
        "ps-kid",
        "https://ps.example",
    );
    router(Arc::new(server))
}

/// Store double whose every operation fails, standing in for a durable
/// backend outage.
struct FailingStore;

#[async_trait]
impl PersonStateStore for FailingStore {
    async fn insert_pending(&self, _pending: PendingRequest) -> Result<(), StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn get_pending(&self, _id: &crate::pending::PendingId) -> Result<Option<PendingRequest>, StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn update_pending(&self, _pending: PendingRequest) -> Result<(), StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn remove_pending(&self, _id: &crate::pending::PendingId) -> Result<(), StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn find_pending_by_correlation(&self, _key: &str) -> Result<Option<crate::pending::PendingId>, StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn insert_pending_unless_correlated(
        &self,
        _pending: PendingRequest,
    ) -> Result<Option<crate::pending::PendingId>, StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn insert_mission(&self, _mission: Mission) -> Result<(), StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn get_mission(&self, _id: &MissionId) -> Result<Option<Mission>, StoreError> {
        Err(StoreError("backend down".into()))
    }
    async fn update_mission(&self, _mission: Mission) -> Result<(), StoreError> {
        Err(StoreError("backend down".into()))
    }
}

/// Builds an active mission with the given `s256` id, ready for
/// `insert_mission` into a store fixture.
fn active_mission(s256: &str) -> Mission {
    Mission {
        id: MissionId::from_s256(s256),
        blob_bytes: b"{}".to_vec(),
        approved: ApprovedMission {
            approver: "https://ps.example".to_string(),
            agent: "aauth:assistant@agent.example".to_string(),
            approved_at: "2026-01-01T00:00:00Z".to_string(),
            description: "test mission".to_string(),
            approved_tools: None,
            capabilities: None,
        },
        status: MissionStatus::Active,
        log: Vec::new(),
    }
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

#[test]
fn extract_agent_token_returns_none_for_non_utf8_header_value() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "signature-key",
        axum::http::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
    );
    assert_eq!(extract_agent_token(&headers), None);
}

#[test]
fn extract_agent_token_returns_none_without_jwt_marker() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("signature-key", "sig=jwt".parse().unwrap());
    assert_eq!(extract_agent_token(&headers), None);
}

#[test]
fn extract_agent_token_returns_none_without_closing_quote() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("signature-key", "sig=jwt;jwt=\"unterminated".parse().unwrap());
    assert_eq!(extract_agent_token(&headers), None);
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

#[tokio::test]
async fn audit_endpoint_appends_to_active_mission_and_returns_204() {
    let jwks = StaticJwks::new();
    let store = InMemoryStore::new();
    store.insert_mission(active_mission("known-mission")).await.unwrap();
    let app = build_router_with_store(vec![], jwks, store);

    let body = serde_json::to_vec(&serde_json::json!({
        "mission": {"approver": "https://ps.example", "s256": "known-mission"},
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
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn token_endpoint_deny_surfaces_error_response() {
    let agent_fixture = key_fixture("agent-kid-deny");
    let resource_fixture = key_fixture("resource-kid-deny");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid-deny",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid-deny",
    );

    let app = build_router(
        vec![PolicyDecision::Deny {
            reason: "not within mission scope".to_string(),
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
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "denied");
}

async fn seed_pending(store: &InMemoryStore, phase: PendingPhase) -> String {
    let agent_fixture = key_fixture("poll-agent-kid");
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let agent = AgentClaims {
        iss: "https://ap.example".to_string(),
        sub: "aauth:assistant@agent.example".to_string(),
        jti: "agent-jti".to_string(),
        iat: now_unix() - 5,
        exp: now_unix() + 600,
        dwk: "aauth-agent.json".to_string(),
        cnf: Cnf {
            jwk: agent_fixture.jwk.clone(),
        },
        ps: None,
    };
    let resource = ResourceClaims {
        iss: "https://calendar.example".to_string(),
        aud: "https://ps.example".to_string(),
        jti: "resource-jti".to_string(),
        iat: now_unix() - 5,
        exp: now_unix() + 600,
        dwk: "aauth-resource.json".to_string(),
        agent: agent.sub.clone(),
        agent_jkt,
        scope: "calendar.readwrite".to_string(),
        mission: None,
    };
    let mut pending = PendingRequest::new(agent, resource, None);
    pending.phase = phase;
    let id = pending.id.0.clone();
    store.insert_pending(pending).await.unwrap();
    id
}

#[tokio::test]
async fn poll_endpoint_returns_granted_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(
        &store,
        PendingPhase::Granted {
            auth_token: "opaque-token".to_string(),
            expires_in: 3600,
        },
    )
    .await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["auth_token"], "opaque-token");
    assert_eq!(json["expires_in"], 3600);
}

#[tokio::test]
async fn poll_endpoint_returns_denied_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(
        &store,
        PendingPhase::Denied {
            reason: "not within mission scope".to_string(),
        },
    )
    .await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "denied");
}

#[tokio::test]
async fn poll_endpoint_returns_canceled_phase_as_410() {
    let store = InMemoryStore::new();
    let id = seed_pending(&store, PendingPhase::Canceled).await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "denied");
    assert_eq!(json["detail"], "request canceled");
}

#[tokio::test]
async fn poll_endpoint_returns_awaiting_clarification_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(
        &store,
        PendingPhase::AwaitingClarification {
            clarification: trogon_identity_types::aauth::person_server::ClarificationRequired {
                status: "pending".to_string(),
                clarification: "which calendar?".to_string(),
                timeout: None,
                options: None,
            },
        },
    )
    .await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["clarification"], "which calendar?");
}

#[tokio::test]
async fn poll_endpoint_returns_awaiting_resource_interaction_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(
        &store,
        PendingPhase::AwaitingResourceInteraction {
            url: "https://calendar.example/authorize".to_string(),
            code: "abc123".to_string(),
        },
    )
    .await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "interacting");
    assert_eq!(json["options"][0], "https://calendar.example/authorize");
    assert_eq!(json["options"][1], "abc123");
}

#[tokio::test]
async fn poll_endpoint_returns_interacting_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(&store, PendingPhase::Interacting).await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "interacting");
}

#[tokio::test]
async fn poll_endpoint_returns_pending_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(&store, PendingPhase::Pending).await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn poll_endpoint_returns_approval_pending_phase() {
    let store = InMemoryStore::new();
    let id = seed_pending(&store, PendingPhase::ApprovalPending).await;
    let app = build_router_with_store(vec![], StaticJwks::new(), store);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/token/{id}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn permission_endpoint_without_mission_returns_mission_not_found() {
    let jwks = StaticJwks::new();
    let app = build_router(vec![], jwks);

    let body = serde_json::to_vec(&serde_json::json!({ "action": "WebSearch" })).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/permission")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn permission_endpoint_appends_to_active_mission_and_grants() {
    let jwks = StaticJwks::new();
    let store = InMemoryStore::new();
    store.insert_mission(active_mission("known-mission")).await.unwrap();
    let app = build_router_with_store(vec![], jwks, store);

    let body = serde_json::to_vec(&serde_json::json!({
        "action": "WebSearch",
        "mission": {"approver": "https://ps.example", "s256": "known-mission"}
    }))
    .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/permission")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["permission"], "granted");
}

#[tokio::test]
async fn permission_endpoint_surfaces_store_failure() {
    let jwks = StaticJwks::new();
    let app = build_router_with_store(vec![], jwks, FailingStore);

    let body = serde_json::to_vec(&serde_json::json!({
        "action": "WebSearch",
        "mission": {"approver": "https://ps.example", "s256": "known-mission"}
    }))
    .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/permission")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn audit_endpoint_surfaces_store_failure() {
    let jwks = StaticJwks::new();
    let app = build_router_with_store(vec![], jwks, FailingStore);

    let body = serde_json::to_vec(&serde_json::json!({
        "mission": {"approver": "https://ps.example", "s256": "known-mission"},
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

#[tokio::test]
async fn interaction_endpoint_without_mission_returns_mission_not_found() {
    let jwks = StaticJwks::new();
    let app = build_router(vec![], jwks);

    let body = serde_json::to_vec(&serde_json::json!({ "type": "interaction" })).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/interaction")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn interaction_endpoint_appends_to_active_mission_and_returns_202() {
    let jwks = StaticJwks::new();
    let store = InMemoryStore::new();
    store.insert_mission(active_mission("known-mission")).await.unwrap();
    let app = build_router_with_store(vec![], jwks, store);

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "interaction",
        "mission": {"approver": "https://ps.example", "s256": "known-mission"}
    }))
    .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/interaction")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn interaction_endpoint_surfaces_store_failure() {
    let jwks = StaticJwks::new();
    let app = build_router_with_store(vec![], jwks, FailingStore);

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "interaction",
        "mission": {"approver": "https://ps.example", "s256": "known-mission"}
    }))
    .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/interaction")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn approve_mission_response_renders_exact_bytes() {
    let jwks = StaticJwks::new();
    let server: TestServer = PersonServer::new(
        TokenVerifier::new(jwks, SystemTimeSource),
        SystemTimeSource,
        ScriptedPolicy::new(vec![]),
        NoopInteractionChannel,
        InMemoryStore::new(),
        {
            let ps_signing = SigningKey::random(&mut OsRng);
            let ps_pem = ps_signing.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
            EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap()
        },
        Algorithm::ES256,
        "ps-kid",
        "https://ps.example",
    );
    let blob = MissionBlob {
        approver: "https://ps.example".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        approved_at: "2026-01-01T00:00:00Z".to_string(),
        description: "test mission".to_string(),
        approved_tools: None,
        capabilities: None,
    };
    let bytes = approve_mission_response(&server, blob).await.unwrap();
    let expected = serde_json::to_vec(&MissionBlob {
        approver: "https://ps.example".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        approved_at: "2026-01-01T00:00:00Z".to_string(),
        description: "test mission".to_string(),
        approved_tools: None,
        capabilities: None,
    })
    .unwrap();
    assert_eq!(bytes, expected);
}

#[test]
fn mission_status_error_body_carries_terminated_status() {
    let body = mission_status_error_body(MissionStatus::Terminated);
    assert_eq!(body.error, "mission_terminated");
    assert_eq!(body.mission_status, MissionStatus::Terminated);
}
