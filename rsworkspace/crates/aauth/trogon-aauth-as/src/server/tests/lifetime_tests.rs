use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use jsonwebtoken::{Header, encode};
use trogon_aauth_verify::{TimeSource, TokenError};
use trogon_identity_types::aauth::{DWK_AGENT, DWK_RESOURCE, TYP_AGENT, TYP_AUTH, TYP_RESOURCE};

use super::*;
use crate::test_support::KeyFixture;

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
}

impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            agent: 600,
            resource: 600,
            subagent: None,
            upstream: None,
        }
    }
}

fn signed(fixture: &KeyFixture, typ: &str, mut claims: serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(typ.to_string());
    header.kid = fixture.jwk.common.key_id.clone();
    claims["iat"] = (NOW - 1).into();
    claims["jti"] = uuid::Uuid::new_v4().to_string().into();
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

fn fixture(
    scope: &str,
    lifetimes: Lifetimes,
) -> (
    AccessServer<StaticJwks, Clock, ScopeSwitchedPolicy>,
    AsTokenRequest,
    Clock,
) {
    fixture_with_policy(scope, lifetimes, Clock::new(), ScopeSwitchedPolicy)
}

fn fixture_with_policy<P: OrganizationPolicy>(
    scope: &str,
    lifetimes: Lifetimes,
    clock: Clock,
    policy: P,
) -> (AccessServer<StaticJwks, Clock, P>, AsTokenRequest, Clock) {
    let issuer = key_fixture("lifetime-as");
    let provider = key_fixture("lifetime-ap");
    let resource = key_fixture("lifetime-resource");
    let agent = key_fixture("lifetime-agent");
    let subagent = key_fixture("lifetime-subagent");
    let upstream = key_fixture("lifetime-upstream");
    let agent_token = signed(
        &provider,
        TYP_AGENT,
        serde_json::json!({
            "iss": AP_ISS, "sub": AGENT, "exp": NOW + lifetimes.agent,
            "dwk": DWK_AGENT, "cnf": {"jwk": agent.jwk_json},
        }),
    );
    let subagent_token = lifetimes.subagent.map(|ttl| {
        signed(
            &provider,
            TYP_AGENT,
            serde_json::json!({
                "iss": AP_ISS, "sub": SUBAGENT, "exp": NOW + ttl,
                "dwk": DWK_AGENT, "cnf": {"jwk": subagent.jwk_json}, "parent_agent": AGENT,
            }),
        )
    });
    let (bound_agent, bound_key) = match lifetimes.subagent {
        Some(_) => (SUBAGENT, &subagent.jwk_json),
        None => (AGENT, &agent.jwk_json),
    };
    let resource_token = signed(
        &resource,
        TYP_RESOURCE,
        serde_json::json!({
            "iss": RESOURCE_ISS, "aud": AS_ISS, "exp": NOW + lifetimes.resource,
            "dwk": DWK_RESOURCE, "agent": bound_agent,
            "agent_jkt": trogon_aauth_verify::jwk_thumbprint(bound_key).unwrap(), "scope": scope,
        }),
    );
    let upstream_token = lifetimes.upstream.map(|ttl| {
        signed(
            &upstream,
            TYP_AUTH,
            serde_json::json!({
                "iss": PS_ISS, "aud": AP_ISS, "sub": "person:alice", "exp": NOW + ttl,
                "agent": AGENT, "agent_jkt": trogon_aauth_verify::jwk_thumbprint(&agent.jwk_json).unwrap(),
                "scope": "documents:read", "cnf": {"jwk": agent.jwk_json},
            }),
        )
    });
    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&provider]))
        .with(RESOURCE_ISS, jwk_set(&[&resource]))
        .with(PS_ISS, jwk_set(&[&upstream]));
    let verifier = TokenVerifier::new(jwks, clock.clone());
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let config = AccessServerConfig {
        iss: AS_ISS.to_string(),
        signing_key: issuer.encoding_key,
        alg: Algorithm::ES256,
        kid: "lifetime-as".to_string(),
        auth_token_ttl: AuthTokenTtl::new(300).unwrap(),
        mode: FederationMode::Federated,
    };
    let server = AccessServer::new(verifier, clock.clone(), trust, policy, config);
    let request = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token,
        upstream_token,
    };
    (server, request, clock)
}

async fn assert_grant_lifetime(lifetimes: Lifetimes, expected: i64) {
    let (server, request, _) = fixture("documents:read", lifetimes);
    let outcome = server.evaluate(PS_ISS, &request).await.unwrap();
    let AsOutcome::Issued(response) = outcome else {
        panic!("expected a grant");
    };
    assert_eq!(response.expires_in, expected);
    assert_eq!(decode_claims_unverified(&response.auth_token)["exp"], NOW + expected);
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
        300,
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
    let (server, request, clock) = fixture("claims-please", lifetimes);
    let outcome = server.evaluate(PS_ISS, &request).await.unwrap();
    let AsOutcome::ClaimsRequired { pending_id, .. } = outcome else {
        panic!("expected a claims round trip");
    };
    clock.set(NOW + 60);
    let claims = ClaimsSubmission {
        sub: "person:alice".into(),
        claims: serde_json::Map::new(),
    };
    let result = server.resume_with_claims(&pending_id, PS_ISS, &claims).await;
    let matches = match expected {
        ExpiredToken::Agent => matches!(
            result,
            Err(AccessServerError::Verification(RequestVerificationError::AgentToken(
                TokenError::Expired
            )))
        ),
        ExpiredToken::Resource => matches!(
            result,
            Err(AccessServerError::Verification(
                RequestVerificationError::ResourceToken(TokenError::Expired)
            ))
        ),
        ExpiredToken::Subagent => matches!(
            result,
            Err(AccessServerError::Verification(
                RequestVerificationError::SubagentToken(TokenError::Expired)
            ))
        ),
        ExpiredToken::Upstream => matches!(
            result,
            Err(AccessServerError::Verification(
                RequestVerificationError::UpstreamToken(TokenError::Expired)
            ))
        ),
    };
    assert!(matches, "expired request authority must not resume");
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
    let (server, request, clock) = fixture(
        "claims-please",
        Lifetimes {
            agent: 60,
            ..Lifetimes::default()
        },
    );
    let outcome = server.evaluate(PS_ISS, &request).await.unwrap();
    let AsOutcome::ClaimsRequired { pending_id, .. } = outcome else {
        panic!("expected claims")
    };
    clock.set(NOW + 20);
    let claims = ClaimsSubmission {
        sub: "person:alice".into(),
        claims: serde_json::Map::new(),
    };
    let result = server.resume_with_claims(&pending_id, PS_ISS, &claims).await.unwrap();
    let AsOutcome::Issued(response) = result else {
        panic!("expected a grant")
    };
    assert_eq!(response.expires_in, 40);
    assert_eq!(decode_claims_unverified(&response.auth_token)["exp"], NOW + 60);
}

struct ExpiringPolicy(Clock);

impl OrganizationPolicy for ExpiringPolicy {
    fn decide(&self, ctx: &AsTokenContext<'_>, _trust: &TrustedIssuer) -> Decision {
        self.0.set(NOW + 60);
        Decision::Issue {
            scope: GrantedScope::new(ctx.resource_claims.scope.clone()),
        }
    }

    fn decide_with_claims(
        &self,
        ctx: &AsTokenContext<'_>,
        trust: &TrustedIssuer,
        _claims: &ClaimsSubmission,
    ) -> Decision {
        self.decide(ctx, trust)
    }
}

#[tokio::test]
async fn grant_rechecks_authority_after_policy() {
    let clock = Clock::new();
    let (server, request, _) = fixture_with_policy(
        "documents:read",
        Lifetimes {
            agent: 60,
            ..Lifetimes::default()
        },
        clock.clone(),
        ExpiringPolicy(clock),
    );
    let result = server.evaluate(PS_ISS, &request).await;
    assert!(matches!(
        result,
        Err(AccessServerError::Verification(RequestVerificationError::AgentToken(
            TokenError::Expired
        )))
    ));
}
