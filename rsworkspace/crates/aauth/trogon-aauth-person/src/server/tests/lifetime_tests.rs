use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use trogon_aauth_verify::TokenError;
use trogon_identity_types::aauth::{DWK_AGENT, DWK_RESOURCE, TYP_AUTH};

use super::*;
use crate::error::RequestVerificationError;

const NOW: i64 = 2_000_000_000;
const AGENT: &str = "aauth:assistant@agent.example";
const SUBAGENT: &str = "aauth:assistant+worker@agent.example";

#[derive(Clone)]
struct Clock(Arc<AtomicI64>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(AtomicI64::new(NOW)))
    }

    fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl TimeSource for Clock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Lifetimes {
    agent: i64,
    resource: i64,
    subagent: Option<i64>,
    upstream: Option<i64>,
    resource_agent: Option<&'static str>,
}

impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            agent: 600,
            resource: 600,
            subagent: None,
            upstream: None,
            resource_agent: None,
        }
    }
}

fn signed(fixture: &KeyFixture, kid: &str, typ: &str, mut claims: serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(typ.to_string());
    header.kid = Some(kid.to_string());
    claims["iat"] = (NOW - 1).into();
    claims["jti"] = uuid::Uuid::new_v4().to_string().into();
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

type LifetimeServer<P = ScriptedPolicy> = PersonServer<StaticJwks, Clock, P, NoopInteractionChannel, InMemoryStore>;

fn fixture(lifetimes: Lifetimes, decisions: Vec<PolicyDecision>) -> (LifetimeServer, String, TokenRequest, Clock) {
    fixture_with_policy(lifetimes, Clock::new(), ScriptedPolicy::new(decisions))
}

fn fixture_with_policy<P: PolicyEngine>(
    lifetimes: Lifetimes,
    clock: Clock,
    policy: P,
) -> (LifetimeServer<P>, String, TokenRequest, Clock) {
    let provider = key_fixture("lifetime-ap");
    let resource = key_fixture("lifetime-resource");
    let agent = key_fixture("lifetime-agent");
    let subagent = key_fixture("lifetime-subagent");
    let upstream = key_fixture("lifetime-upstream");
    let issuer = key_fixture("lifetime-ps");
    let agent_token = signed(
        &provider,
        "lifetime-ap",
        TYP_AGENT,
        serde_json::json!({
            "iss": "https://ap.example", "sub": AGENT, "exp": NOW + lifetimes.agent,
            "dwk": DWK_AGENT, "cnf": {"jwk": agent.jwk},
        }),
    );
    let subagent_token = lifetimes.subagent.map(|ttl| {
        signed(
            &provider,
            "lifetime-ap",
            TYP_AGENT,
            serde_json::json!({
                "iss": "https://ap.example", "sub": SUBAGENT, "exp": NOW + ttl,
                "dwk": DWK_AGENT, "cnf": {"jwk": subagent.jwk}, "parent_agent": AGENT,
            }),
        )
    });
    let (bound_agent, bound_key) = match lifetimes.subagent {
        Some(_) => (SUBAGENT, &subagent.jwk),
        None => (AGENT, &agent.jwk),
    };
    let resource_token = signed(
        &resource,
        "lifetime-resource",
        TYP_RESOURCE,
        serde_json::json!({
            "iss": "https://resource.example", "aud": "https://ps.example", "exp": NOW + lifetimes.resource,
            "dwk": DWK_RESOURCE, "agent": lifetimes.resource_agent.unwrap_or(bound_agent), "agent_jkt": jkt_of(bound_key), "scope": "documents:read",
        }),
    );
    let upstream_token = lifetimes.upstream.map(|ttl| {
        signed(
            &upstream,
            "lifetime-upstream",
            TYP_AUTH,
            serde_json::json!({
                "iss": "https://upstream.example", "aud": "https://ap.example", "sub": "person:alice", "exp": NOW + ttl,
                "agent": AGENT, "agent_jkt": jkt_of(&agent.jwk), "scope": "documents:read", "cnf": {"jwk": agent.jwk},
                "act": {"agent": "aauth:upstream@agent.example"},
            }),
        )
    });
    let jwks = StaticJwks::new()
        .with("https://ap.example", provider.jwk_set)
        .with("https://resource.example", resource.jwk_set)
        .with("https://upstream.example", upstream.jwk_set);
    let server = PersonServer::new(
        TokenVerifier::new(jwks, clock.clone()),
        clock.clone(),
        policy,
        NoopInteractionChannel,
        InMemoryStore::new(),
        issuer.encoding_key,
        Algorithm::ES256,
        "lifetime-ps",
        "https://ps.example",
    );
    let request = TokenRequest {
        resource_token,
        subagent_token,
        upstream_token,
        ..TokenRequest::new("")
    };
    (server, agent_token, request, clock)
}

fn grant() -> PolicyDecision {
    PolicyDecision::Grant {
        scope: "documents:read".to_string(),
    }
}

fn clarification() -> PolicyDecision {
    PolicyDecision::NeedsClarification {
        clarification: "why?".to_string(),
        options: None,
    }
}

fn claims(jwt: &str) -> serde_json::Value {
    let payload = jwt.split('.').nth(1).unwrap();
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) fn expires_at(jwt: &str) -> i64 {
    claims(jwt)["exp"].as_i64().unwrap()
}

async fn assert_grant_lifetime(lifetimes: Lifetimes, expected: i64) {
    let (server, agent, request, _) = fixture(lifetimes, vec![grant()]);
    let outcome = server.evaluate_token_request(&agent, &request).await.unwrap();
    let TokenEndpointOutcome::Grant { response } = outcome else {
        panic!("expected a grant")
    };
    assert_eq!(response.expires_in, expected);
    assert_eq!(claims(&response.auth_token)["exp"], NOW + expected);
}

#[tokio::test]
async fn grant_is_bounded_by_agent_expiry() {
    assert_grant_lifetime(
        Lifetimes {
            agent: 60,
            ..Lifetimes::default()
        },
        60,
    )
    .await;
}

#[tokio::test]
async fn grant_is_bounded_by_subagent_expiry() {
    assert_grant_lifetime(
        Lifetimes {
            subagent: Some(60),
            ..Lifetimes::default()
        },
        60,
    )
    .await;
}

#[tokio::test]
async fn grant_is_bounded_by_upstream_authority_expiry() {
    assert_grant_lifetime(
        Lifetimes {
            upstream: Some(60),
            ..Lifetimes::default()
        },
        60,
    )
    .await;
}

#[tokio::test]
async fn resource_challenge_expiry_does_not_limit_grant_lifetime() {
    assert_grant_lifetime(
        Lifetimes {
            resource: 10,
            ..Lifetimes::default()
        },
        600,
    )
    .await;
}

#[derive(Clone, Copy)]
enum ExpiredToken {
    Agent,
    Resource,
    Subagent,
    Upstream,
}

async fn assert_expired_resume(lifetimes: Lifetimes, expected: ExpiredToken) {
    let (server, agent, request, clock) = fixture(lifetimes, vec![clarification(), grant()]);
    let outcome = server.evaluate_token_request(&agent, &request).await.unwrap();
    let TokenEndpointOutcome::Pending { pending_id, .. } = outcome else {
        panic!("expected clarification")
    };
    clock.set(NOW + 60);
    let result = server
        .respond_to_clarification(
            &pending_id,
            ClarificationAction::ClarificationResponse,
            Some("because"),
            None,
        )
        .await;
    let matches = match expected {
        ExpiredToken::Agent => matches!(
            result,
            Err(PersonServerError::Verification(RequestVerificationError::AgentToken(
                TokenError::Expired
            )))
        ),
        ExpiredToken::Resource => matches!(
            result,
            Err(PersonServerError::Verification(
                RequestVerificationError::ResourceToken(TokenError::Expired)
            ))
        ),
        ExpiredToken::Subagent => matches!(
            result,
            Err(PersonServerError::Verification(
                RequestVerificationError::SubagentToken(TokenError::Expired)
            ))
        ),
        ExpiredToken::Upstream => matches!(
            result,
            Err(PersonServerError::Verification(
                RequestVerificationError::UpstreamToken(TokenError::Expired)
            ))
        ),
    };
    assert!(matches, "expired request authority must not resume");
    assert!(
        server.poll_pending(&pending_id).await.unwrap().is_terminal(),
        "expired request must release its pending correlation"
    );
}

#[tokio::test]
async fn resource_agent_must_match_presenting_agent_before_parking() {
    let (server, agent, request, _) = fixture(
        Lifetimes {
            resource_agent: Some("aauth:another@agent.example"),
            ..Lifetimes::default()
        },
        vec![clarification(), grant()],
    );
    assert!(matches!(
        server.evaluate_token_request(&agent, &request).await,
        Err(PersonServerError::Verification(
            RequestVerificationError::ResourceTokenAgentIdentifierMismatch
        ))
    ));
}

#[tokio::test]
async fn pending_resume_rejects_expired_agent() {
    assert_expired_resume(
        Lifetimes {
            agent: 60,
            ..Lifetimes::default()
        },
        ExpiredToken::Agent,
    )
    .await;
}

#[tokio::test]
async fn pending_resume_rejects_expired_resource_challenge() {
    assert_expired_resume(
        Lifetimes {
            resource: 60,
            ..Lifetimes::default()
        },
        ExpiredToken::Resource,
    )
    .await;
}

#[tokio::test]
async fn pending_resume_rejects_expired_subagent() {
    assert_expired_resume(
        Lifetimes {
            subagent: Some(60),
            ..Lifetimes::default()
        },
        ExpiredToken::Subagent,
    )
    .await;
}

#[tokio::test]
async fn pending_resume_rejects_expired_upstream_authority() {
    assert_expired_resume(
        Lifetimes {
            upstream: Some(60),
            ..Lifetimes::default()
        },
        ExpiredToken::Upstream,
    )
    .await;
}

#[tokio::test]
async fn resumed_grant_uses_remaining_authority_lifetime() {
    let (server, agent, request, clock) = fixture(
        Lifetimes {
            agent: 60,
            ..Lifetimes::default()
        },
        vec![clarification(), grant()],
    );
    let outcome = server.evaluate_token_request(&agent, &request).await.unwrap();
    let TokenEndpointOutcome::Pending { pending_id, .. } = outcome else {
        panic!("expected clarification")
    };
    clock.set(NOW + 20);
    let result = server
        .respond_to_clarification(
            &pending_id,
            ClarificationAction::ClarificationResponse,
            Some("because"),
            None,
        )
        .await
        .unwrap();
    let TokenEndpointOutcome::Grant { response } = result else {
        panic!("expected a grant")
    };
    assert_eq!(response.expires_in, 40);
    assert_eq!(claims(&response.auth_token)["exp"], NOW + 60);
}

struct ExpiringPolicy(Clock);

#[async_trait]
impl PolicyEngine for ExpiringPolicy {
    type Error = StubPolicyError;

    async fn decide(&self, _request: DecisionRequest<'_>) -> Result<PolicyDecision, Self::Error> {
        self.0.set(NOW + 60);
        Ok(grant())
    }
}

#[tokio::test]
async fn grant_rechecks_authority_after_async_policy() {
    let clock = Clock::new();
    let (server, agent, request, _) = fixture_with_policy(
        Lifetimes {
            agent: 60,
            ..Lifetimes::default()
        },
        clock.clone(),
        ExpiringPolicy(clock),
    );
    let result = server.evaluate_token_request(&agent, &request).await;
    assert!(matches!(
        result,
        Err(PersonServerError::Verification(RequestVerificationError::AgentToken(
            TokenError::Expired
        )))
    ));
}

#[tokio::test]
async fn subagent_grant_records_parent_attribution() {
    let (server, agent, request, _) = fixture(
        Lifetimes {
            subagent: Some(600),
            ..Lifetimes::default()
        },
        vec![grant()],
    );
    let result = server.evaluate_token_request(&agent, &request).await.unwrap();
    let TokenEndpointOutcome::Grant { response } = result else {
        panic!("expected a grant")
    };
    let claims = claims(&response.auth_token);
    assert_eq!(claims["agent"], SUBAGENT);
    assert_eq!(claims["act"]["agent"], AGENT);
}

#[tokio::test]
async fn resumed_subagent_grant_preserves_parent_and_upstream_attribution() {
    let (server, agent, request, _) = fixture(
        Lifetimes {
            subagent: Some(600),
            upstream: Some(600),
            ..Lifetimes::default()
        },
        vec![clarification(), grant()],
    );
    let result = server.evaluate_token_request(&agent, &request).await.unwrap();
    let TokenEndpointOutcome::Pending { pending_id, .. } = result else {
        panic!("expected clarification")
    };
    let result = server
        .respond_to_clarification(
            &pending_id,
            ClarificationAction::ClarificationResponse,
            Some("because"),
            None,
        )
        .await
        .unwrap();
    let TokenEndpointOutcome::Grant { response } = result else {
        panic!("expected a grant")
    };
    let claims = claims(&response.auth_token);
    assert_eq!(claims["agent"], SUBAGENT);
    assert_eq!(claims["act"]["agent"], AGENT);
    assert_eq!(claims["act"]["act"]["agent"], "aauth:upstream@agent.example");
}
