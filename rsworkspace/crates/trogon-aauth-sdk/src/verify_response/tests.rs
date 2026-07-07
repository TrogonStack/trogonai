#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::SigningKey;
use rand_core::OsRng;
use trogon_identity_types::aauth::{Act, Cnf};

use super::*;

const RESOURCE_TOKEN_AUD: &str = "https://ps.example";
const RESOURCE: &str = "https://calendar.example";
const OWN_AGENT: &str = "aauth:asst@agent.example";

fn own_jwk() -> serde_json::Value {
    let sk = SigningKey::random(&mut OsRng);
    let point = sk.verifying_key().to_encoded_point(false);
    serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": URL_SAFE_NO_PAD.encode(point.x().expect("x")),
        "y": URL_SAFE_NO_PAD.encode(point.y().expect("y")),
    })
}

fn valid_claims(agent_jwk: serde_json::Value) -> AuthClaims {
    AuthClaims {
        iss: RESOURCE_TOKEN_AUD.to_string(),
        sub: "user-1".to_string(),
        aud: RESOURCE.to_string(),
        jti: "auth-jti-1".to_string(),
        iat: 1_700_000_000,
        exp: 1_700_003_600,
        agent: OWN_AGENT.to_string(),
        agent_jkt: "unused-legacy-field".to_string(),
        scope: "calendar.read".to_string(),
        principal: None,
        consent_id: None,
        resource: None,
        act: None,
        cnf: Some(Cnf { jwk: agent_jwk }),
    }
}

#[test]
fn all_rules_pass_positive_case() {
    let jwk = own_jwk();
    let claims = valid_claims(jwk.clone());
    let result = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None)
        .expect("all rules should pass");
    assert_eq!(result.upstream_agent, None);
}

#[test]
fn rule2_fails_when_iss_does_not_match_resource_token_aud() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.iss = "https://wrong-ps.example".to_string();
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::IssuerMismatch { .. }));
}

#[test]
fn rule3_fails_when_aud_does_not_match_intended_resource() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.aud = "https://other-resource.example".to_string();
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::AudienceMismatch { .. }));
}

#[test]
fn rule4_fails_when_cnf_jwk_does_not_match_agents_own_key() {
    let jwk = own_jwk();
    let other_jwk = own_jwk();
    let claims = valid_claims(other_jwk);
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::ConfirmationKeyMismatch));
}

#[test]
fn rule4_fails_when_cnf_claim_is_missing() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.cnf = None;
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::ConfirmationClaimMissing));
}

#[test]
fn rule5_fails_when_agent_does_not_match_own_identifier() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.agent = "aauth:someone-else@agent.example".to_string();
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::AgentMismatch { .. }));
}

#[test]
fn rule6_fails_when_act_agent_does_not_match_expected_upstream() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.act = Some(Act {
        agent: "aauth:booking@booking.example".to_string(),
        act: None,
    });
    let err = verify_auth_claims(
        &claims,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk,
        OWN_AGENT,
        Some("aauth:planner@agent.example"),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyResponseError::UpstreamAgentMismatch { .. }));
}

#[test]
fn rule6_passes_and_surfaces_upstream_agent_when_expected_matches() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.act = Some(Act {
        agent: "aauth:booking@booking.example".to_string(),
        act: None,
    });
    let result = verify_auth_claims(
        &claims,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk,
        OWN_AGENT,
        Some("aauth:booking@booking.example"),
    )
    .expect("matching expected upstream agent should pass");
    assert_eq!(result.upstream_agent.as_deref(), Some("aauth:booking@booking.example"));
}

#[test]
fn rule6_does_not_fail_when_act_present_but_no_expectation_supplied() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.act = Some(Act {
        agent: "aauth:booking@booking.example".to_string(),
        act: None,
    });
    let result = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None)
        .expect("no expectation supplied means rule 6 is not enforced");
    assert_eq!(result.upstream_agent.as_deref(), Some("aauth:booking@booking.example"));
}
