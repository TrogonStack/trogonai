#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;

fn fake_jwt(claims: &serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(serde_json::json!({"alg": "ES256", "typ": "aa-agent+jwt"}).to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    format!("{header}.{payload}.sig")
}

fn root_agent_jwt() -> String {
    fake_jwt(&serde_json::json!({
        "iss": "https://ap.example",
        "sub": "aauth:asst@agent.example",
        "jti": "jti-1",
        "iat": 1_700_000_000,
        "exp": 1_700_003_600,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": {"kty": "EC", "crv": "P-256", "x": "x", "y": "y"}},
    }))
}

fn subagent_jwt() -> String {
    fake_jwt(&serde_json::json!({
        "iss": "https://ap.example",
        "sub": "aauth:asst+search1@agent.example",
        "jti": "jti-2",
        "iat": 1_700_000_000,
        "exp": 1_700_003_600,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": {"kty": "EC", "crv": "P-256", "x": "x2", "y": "y2"}},
        "parent_agent": "aauth:asst@agent.example",
    }))
}

#[test]
fn build_token_request_sets_upstream_token_for_call_chaining() {
    let agent_jwt = root_agent_jwt();
    let request = build_token_request(
        &agent_jwt,
        "downstream-resource-token",
        Some("upstream-auth-token".to_string()),
    )
    .expect("root agent may build a token request");
    assert_eq!(request.resource_token, "downstream-resource-token");
    assert_eq!(request.upstream_token.as_deref(), Some("upstream-auth-token"));
}

#[test]
fn build_token_request_without_upstream_token_leaves_it_unset() {
    let agent_jwt = root_agent_jwt();
    let request = build_token_request(&agent_jwt, "resource-token", None).expect("root agent may build a request");
    assert_eq!(request.upstream_token, None);
}

#[test]
fn build_token_request_rejects_subagent_signer() {
    let agent_jwt = subagent_jwt();
    let err = build_token_request(&agent_jwt, "resource-token", None).unwrap_err();
    assert!(matches!(err, SubAgentError::ParentIsSubAgent));
}

#[test]
fn route_for_call_chaining_prefers_mission_approver_over_iss() {
    assert_eq!(
        route_for_call_chaining("https://ps.example", Some("https://approver.example")),
        "https://approver.example"
    );
}

#[test]
fn route_for_call_chaining_falls_back_to_iss_when_no_approver() {
    assert_eq!(
        route_for_call_chaining("https://ps.example", None),
        "https://ps.example"
    );
}
