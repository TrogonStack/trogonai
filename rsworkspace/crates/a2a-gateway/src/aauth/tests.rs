use trogon_aauth_verify::{StaticJwks, TokenError};

use super::*;

fn deny(reason: AAuthDenyReason, challenge: Option<String>) -> AAuthDeny {
    AAuthDeny {
        code: AAUTH_REQUIRED_CODE,
        reason,
        challenge,
    }
}

fn unsupported_token_error() -> TokenError {
    // Pick any TokenError variant — the deny carries it as a source.
    TokenError::BadHeader
}

#[test]
fn deny_renders_requirement_header_when_challenge_present() {
    let d = deny(AAuthDenyReason::Auth(unsupported_token_error()), Some("tok123".into()));
    let (name, value) = d.to_requirement_header().expect("header");
    assert_eq!(name, aauth_headers::REQUIREMENT);
    assert!(value.contains("tok123"));
    assert!(value.starts_with("requirement=auth-token"));
}

#[test]
fn deny_without_challenge_has_no_header() {
    let d = deny(AAuthDenyReason::Auth(unsupported_token_error()), None);
    assert!(d.to_requirement_header().is_none());
}

#[test]
fn deny_display_carries_reason_source_chain() {
    use std::error::Error;
    let d = deny(AAuthDenyReason::Auth(unsupported_token_error()), None);
    let display = format!("{d}");
    assert!(display.contains("aauth denied"));
    let source = d.source().expect("typed source");
    let chained = format!("{source}");
    assert!(chained.contains("verification"));
}

#[test]
fn resolution_anonymous_has_no_identity() {
    let res = AAuthResolution::anonymous();
    assert!(res.agent_id.is_none());
    assert!(res.agent_jkt.is_none());
    assert!(res.principal.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_with_off_mode_returns_anonymous() {
    let cfg = AAuthConfig {
        mode: AAuthMode::Off,
        jwks: StaticJwks::new(),
        resource_iss: "ps.example".into(),
        person_server_aud: "ps.example".into(),
        leeway_secs: 0,
        challenge_alg: jsonwebtoken::Algorithm::ES256,
        // Off mode short-circuits before signing, so a stub HMAC key is OK.
        challenge_key: jsonwebtoken::EncodingKey::from_secret(b"ignored-in-off-mode"),
        challenge_kid: "kid-off".into(),
        challenge_ttl_secs: 60,
        max_skew_secs: 60,
    };
    let ingress = AAuthIngress::new_in_memory(cfg);
    let res = ingress
        .resolve_nats("a2a.gateway.bot.message.send", None, b"{}", &[], None)
        .await
        .expect("off-mode short-circuits");
    assert!(res.agent_id.is_none());
}

fn shadow_cfg() -> AAuthConfig<StaticJwks> {
    AAuthConfig {
        mode: AAuthMode::Shadow,
        jwks: StaticJwks::new(),
        resource_iss: "ps.example".into(),
        person_server_aud: "ps.example".into(),
        leeway_secs: 0,
        challenge_alg: jsonwebtoken::Algorithm::ES256,
        challenge_key: jsonwebtoken::EncodingKey::from_secret(b"ignored-in-shadow"),
        challenge_kid: "kid-shadow".into(),
        challenge_ttl_secs: 60,
        max_skew_secs: 60,
    }
}

fn enforce_cfg() -> AAuthConfig<StaticJwks> {
    AAuthConfig {
        mode: AAuthMode::Enforce,
        ..shadow_cfg()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_shadow_mode_swallows_pop_failure_to_anonymous() {
    // Shadow mode logs the deny reason and surfaces an anonymous resolution
    // so production traffic isn't blocked while operators evaluate.
    let ingress = AAuthIngress::new_in_memory(shadow_cfg());
    let res = ingress
        .resolve_nats("a2a.gateway.bot.message.send", None, b"{}", &[], None)
        .await
        .expect("shadow mode never errors");
    assert!(res.agent_id.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_enforce_mode_denies_without_jkt_carries_no_challenge() {
    // PoP fails before any agent identity is known, so deny_or_shadow has
    // no jkt to mint a challenge against. The deny must surface with
    // challenge=None rather than panicking or fabricating one.
    let ingress = AAuthIngress::new_in_memory(enforce_cfg());
    let err = ingress
        .resolve_nats("a2a.gateway.bot.message.send", None, b"{}", &[], None)
        .await
        .expect_err("enforce mode denies");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(err.challenge.is_none());
    assert!(matches!(err.reason, AAuthDenyReason::Pop(_)));
}

#[test]
fn auth_agent_mismatch_variant_carries_both_sides() {
    let reason = AAuthDenyReason::AuthAgentMismatch {
        agent_sub: "agent-A".into(),
        agent_jkt: "jkt-A".into(),
        auth_agent: "agent-B".into(),
        auth_agent_jkt: "jkt-B".into(),
    };
    let display = format!("{reason}");
    assert!(display.contains("agent-A"));
    assert!(display.contains("jkt-A"));
    assert!(display.contains("agent-B"));
    assert!(display.contains("jkt-B"));
}

#[test]
fn challenge_binding_borrows_from_verified_agent() {
    // The binding type only borrows from VerifiedAgent — construction must
    // not allocate or copy. Smoke-test by asserting field equality through
    // the borrow without holding the agent reference past the binding.
    let claims_json = serde_json::json!({
        "iss": "ap.test",
        "sub": "agent-1",
        "jti": "j1",
        "iat": 1000,
        "exp": 2000,
        "dwk": "aa-agent",
        "cnf": {"jwk": {"kty": "EC", "crv": "P-256", "x": "x", "y": "y"}},
    });
    let claims: trogon_identity_types::aauth::AgentClaims = serde_json::from_value(claims_json).expect("claims");
    let agent = trogon_aauth_verify::VerifiedAgent {
        claims,
        jkt: "test-jkt".into(),
        raw_jwt: "raw".into(),
    };
    let binding = ChallengeBinding::from_agent(&agent);
    assert_eq!(binding.agent_sub, "agent-1");
    assert_eq!(binding.agent_jkt, "test-jkt");
}

#[test]
fn uuid_like_is_non_empty_and_prefixed() {
    let v = uuid_like();
    assert!(v.starts_with("jti-"), "got {v}");
    assert!(v.len() > "jti-".len());
}
