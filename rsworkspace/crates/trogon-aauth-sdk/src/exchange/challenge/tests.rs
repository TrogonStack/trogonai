#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;

const RESOURCE: &str = "https://calendar.example";
const OWN_AGENT: &str = "aauth:asst@agent.example";
const OWN_JKT: &str = "own-jkt-value";

fn valid_claims() -> ResourceClaims {
    ResourceClaims {
        iss: RESOURCE.to_string(),
        aud: "https://ps.example".to_string(),
        jti: "resource-jti-1".to_string(),
        iat: 1_700_000_000,
        exp: 1_700_003_600,
        dwk: "aauth-resource.json".to_string(),
        agent: OWN_AGENT.to_string(),
        agent_jkt: OWN_JKT.to_string(),
        scope: "calendar.read".to_string(),
        mission: None,
    }
}

#[test]
fn all_steps_pass_positive_case() {
    let claims = valid_claims();
    assert!(verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).is_ok());
}

#[test]
fn step3_fails_when_iss_does_not_match_requested_resource() {
    let mut claims = valid_claims();
    claims.iss = "https://other-resource.example".to_string();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::IssuerMismatch { .. }));
}

#[test]
fn step4_fails_when_agent_does_not_match_own_identifier() {
    let mut claims = valid_claims();
    claims.agent = "aauth:someone-else@agent.example".to_string();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::AgentMismatch { .. }));
}

#[test]
fn step5_fails_when_agent_jkt_does_not_match_own_jkt() {
    let mut claims = valid_claims();
    claims.agent_jkt = "wrong-jkt".to_string();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::JktMismatch { .. }));
}

#[test]
fn step6_fails_when_exp_is_not_in_the_future() {
    let claims = valid_claims();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, claims.exp + 1).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::Expired { .. }));
}
