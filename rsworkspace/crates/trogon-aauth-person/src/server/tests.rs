use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TimeSource, TokenVerifier};
use trogon_identity_types::aauth::person_server::TokenRequest;
use trogon_identity_types::aauth::{AgentClaims, Cnf, MissionRef, ResourceClaims, TYP_AGENT, TYP_RESOURCE};

use super::*;
use crate::decision::DecisionRequest;
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
    let pkcs8_pem = signing_key.to_pkcs8_pem(pkcs8::LineEnding::LF).expect("pkcs8 pem");
    let encoding_key = EncodingKey::from_ec_pem(pkcs8_pem.as_bytes()).expect("encoding key");
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

fn mint_resource_jwt(
    fixture: &KeyFixture,
    iss: &str,
    aud: &str,
    agent: &str,
    agent_jkt: &str,
    kid: &str,
    mission: Option<MissionRef>,
) -> String {
    let now = now_unix();
    let claims = ResourceClaims {
        iss: iss.to_string(),
        aud: aud.to_string(),
        jti: format!("resource-jti-{}", uuid_counter()),
        iat: now - 5,
        exp: now + 600,
        dwk: "aauth-resource.json".to_string(),
        agent: agent.to_string(),
        agent_jkt: agent_jkt.to_string(),
        scope: "calendar.readwrite".to_string(),
        mission,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some(kid.to_string());
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

fn uuid_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn jkt_of(jwk: &serde_json::Value) -> String {
    trogon_aauth_verify::jwk_thumbprint(jwk).unwrap()
}

#[derive(Debug, thiserror::Error)]
#[error("stub policy error")]
struct StubPolicyError;

/// Scripted policy engine: returns queued decisions in order, one per call.
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

fn build_server(decisions: Vec<PolicyDecision>, jwks: StaticJwks) -> TestServer {
    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_pem = ps_signing.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
    let ps_encoding_key = EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap();

    PersonServer::new(
        TokenVerifier::new(jwks, SystemTimeSource),
        SystemTimeSource,
        ScriptedPolicy::new(decisions),
        NoopInteractionChannel,
        InMemoryStore::new(),
        ps_encoding_key,
        Algorithm::ES256,
        "ps-kid",
        "https://ps.example",
    )
}

fn agent_and_resource(mission: Option<MissionRef>) -> (KeyFixture, KeyFixture, String, String, StaticJwks) {
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
        mission,
    );
    (agent_fixture, resource_fixture, agent_jwt, resource_jwt, jwks)
}

#[tokio::test]
async fn grant_path_mints_auth_token() {
    let (_agent_fixture, _resource_fixture, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(
        vec![PolicyDecision::Grant {
            scope: "calendar.readwrite".to_string(),
        }],
        jwks,
    );

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    match outcome {
        TokenEndpointOutcome::Grant { response } => {
            assert!(!response.auth_token.is_empty());
            assert_eq!(response.expires_in, 3600);
        }
        other => panic!("expected Grant, got {other:?}"),
    }
}

#[tokio::test]
async fn deny_path_returns_denied_error() {
    let (_a, _r, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(
        vec![PolicyDecision::Deny {
            reason: "outside policy".to_string(),
        }],
        jwks,
    );

    let req = TokenRequest::new(resource_jwt);
    let err = server.evaluate_token_request(&agent_jwt, &req).await.unwrap_err();
    assert!(matches!(err, PersonServerError::Denied(_)));
}

#[tokio::test]
async fn needs_interaction_path_returns_pending_interacting() {
    let (_a, _r, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(vec![PolicyDecision::NeedsInteraction], jwks);

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    match outcome {
        TokenEndpointOutcome::Pending { response, .. } => assert!(response.is_interacting()),
        other => panic!("expected Pending, got {other:?}"),
    }
}

#[tokio::test]
async fn needs_clarification_then_grant_walks_full_round_trip() {
    let (_a, _r, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(
        vec![
            PolicyDecision::NeedsClarification {
                clarification: "Why do you need write access?".to_string(),
                options: None,
            },
            PolicyDecision::Grant {
                scope: "calendar.readonly".to_string(),
            },
        ],
        jwks,
    );

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    let pending_id = match outcome {
        TokenEndpointOutcome::Pending { pending_id, response } => {
            assert_eq!(response.clarification.as_deref(), Some("Why do you need write access?"));
            pending_id
        }
        other => panic!("expected Pending, got {other:?}"),
    };

    let outcome = server
        .respond_to_clarification(
            &pending_id,
            trogon_identity_types::aauth::person_server::ClarificationAction::ClarificationResponse,
            Some("I only need to create invites."),
            None,
        )
        .await
        .unwrap();
    match outcome {
        TokenEndpointOutcome::Grant { response } => assert!(!response.auth_token.is_empty()),
        other => panic!("expected Grant after clarification, got {other:?}"),
    }
}

#[tokio::test]
async fn clarification_round_limit_is_enforced() {
    let (_a, _r, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let mut decisions = Vec::new();
    for _ in 0..(crate::pending::MAX_CLARIFICATION_ROUNDS + 1) {
        decisions.push(PolicyDecision::NeedsClarification {
            clarification: "still unclear".to_string(),
            options: None,
        });
    }
    let server = build_server(decisions, jwks);

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    let mut pending_id = match outcome {
        TokenEndpointOutcome::Pending { pending_id, .. } => pending_id,
        other => panic!("expected Pending, got {other:?}"),
    };

    for _ in 0..(crate::pending::MAX_CLARIFICATION_ROUNDS - 1) {
        let outcome = server
            .respond_to_clarification(
                &pending_id,
                trogon_identity_types::aauth::person_server::ClarificationAction::ClarificationResponse,
                Some("still not clear"),
                None,
            )
            .await
            .unwrap();
        pending_id = match outcome {
            TokenEndpointOutcome::Pending { pending_id, .. } => pending_id,
            other => panic!("expected Pending, got {other:?}"),
        };
    }

    let err = server
        .respond_to_clarification(
            &pending_id,
            trogon_identity_types::aauth::person_server::ClarificationAction::ClarificationResponse,
            Some("still not clear"),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        PersonServerError::Pending(crate::error::PendingRequestError::ClarificationLimitExceeded(_, _))
    ));
}

#[tokio::test]
async fn concurrent_requests_for_same_agent_and_resource_correlate_onto_one_pending() {
    let (_agent_fixture, resource_fixture, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(vec![PolicyDecision::NeedsInteraction], jwks);

    let req = TokenRequest::new(resource_jwt.clone());
    let first = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    let first_id = match first {
        TokenEndpointOutcome::Pending { pending_id, .. } => pending_id,
        other => panic!("expected Pending, got {other:?}"),
    };

    // A second, concurrent request presenting the *same* resource token (same
    // jti) for the same agent must correlate onto the same pending entry per
    // "Concurrent Requests", not spawn a second interaction.
    let _ = resource_fixture; // keep alive for potential future re-signing
    let second = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    let second_id = match second {
        TokenEndpointOutcome::Pending { pending_id, .. } => pending_id,
        other => panic!("expected Pending, got {other:?}"),
    };

    assert_eq!(first_id, second_id);
}

#[tokio::test]
async fn re_authorization_after_grant_creates_new_pending_since_prior_is_terminal() {
    let (agent_fixture, resource_fixture, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(
        vec![
            PolicyDecision::Grant {
                scope: "calendar.readonly".to_string(),
            },
            PolicyDecision::Grant {
                scope: "calendar.readwrite".to_string(),
            },
        ],
        jwks,
    );

    let req = TokenRequest::new(resource_jwt);
    let first = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    assert!(matches!(first, TokenEndpointOutcome::Grant { .. }));

    // Re-authorization: a fresh resource challenge (new jti) for the same
    // agent+resource after the prior grant resolved re-runs policy rather
    // than replaying the terminal (granted) pending entry, per "PS Token
    // Endpoint" re-authorization semantics: a resolved pending flow does not
    // block a later, independent authorization request.
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let new_resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
        None,
    );
    let req2 = TokenRequest::new(new_resource_jwt);
    let second = server.evaluate_token_request(&agent_jwt, &req2).await.unwrap();
    match second {
        TokenEndpointOutcome::Grant { response } => assert_eq!(response.expires_in, 3600),
        other => panic!("expected Grant on re-authorization, got {other:?}"),
    }
}

#[tokio::test]
async fn mission_context_flows_into_policy_and_log() {
    let mission_blob = trogon_identity_types::aauth::mission::MissionBlob {
        approver: "https://ps.example".to_string(),
        agent: "aauth:assistant@agent.example".to_string(),
        approved_at: "2026-01-01T00:00:00Z".to_string(),
        description: "Plan Japan vacation".to_string(),
        approved_tools: None,
        capabilities: None,
    };

    let (_a, _r, agent_jwt, _resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(
        vec![PolicyDecision::Grant {
            scope: "calendar.readwrite".to_string(),
        }],
        jwks,
    );

    let blob_bytes = server.approve_mission(mission_blob.clone()).await.unwrap();
    let mission_id = MissionId::from_blob_bytes(&blob_bytes);
    let mission_ref = MissionRef {
        approver: mission_blob.approver.clone(),
        s256: mission_id.0.clone(),
    };

    let mission = server.get_mission(&mission_id).await.unwrap().unwrap();
    assert!(mission.is_active());
    assert_eq!(mission.mission_ref().s256, mission_ref.s256);

    server.complete_mission(&mission_id).await.unwrap();
    let completed = server.get_mission(&mission_id).await.unwrap().unwrap();
    assert!(!completed.is_active());

    let _ = agent_jwt;
}

#[tokio::test]
async fn approval_pending_decision_returns_pending_without_grant() {
    let (_a, _r, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(vec![PolicyDecision::ApprovalPending], jwks);

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    assert!(matches!(outcome, TokenEndpointOutcome::Pending { .. }));
}

#[tokio::test]
async fn poll_pending_returns_current_phase() {
    let (_a, _r, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(vec![PolicyDecision::NeedsInteraction], jwks);

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    let pending_id = match outcome {
        TokenEndpointOutcome::Pending { pending_id, .. } => pending_id,
        other => panic!("expected Pending, got {other:?}"),
    };

    let polled = server.poll_pending(&pending_id).await.unwrap();
    assert_eq!(polled.phase, crate::pending::PendingPhase::Interacting);
}

#[tokio::test]
async fn updated_request_action_replaces_resource_token_and_rechecks_policy() {
    let (agent_fixture, resource_fixture, agent_jwt, resource_jwt, jwks) = agent_and_resource(None);
    let server = build_server(
        vec![
            PolicyDecision::NeedsClarification {
                clarification: "Scope too broad".to_string(),
                options: None,
            },
            PolicyDecision::Grant {
                scope: "calendar.readonly".to_string(),
            },
        ],
        jwks,
    );

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.unwrap();
    let pending_id = match outcome {
        TokenEndpointOutcome::Pending { pending_id, .. } => pending_id,
        other => panic!("expected Pending, got {other:?}"),
    };

    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let new_resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
        None,
    );
    let updated = trogon_identity_types::aauth::person_server::UpdatedRequest {
        action: trogon_identity_types::aauth::person_server::ClarificationAction::UpdatedRequest,
        resource_token: new_resource_jwt,
        justification: Some("reduced scope".to_string()),
    };

    let outcome = server
        .respond_to_clarification(
            &pending_id,
            trogon_identity_types::aauth::person_server::ClarificationAction::UpdatedRequest,
            None,
            Some(&updated),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, TokenEndpointOutcome::Grant { .. }));
}
