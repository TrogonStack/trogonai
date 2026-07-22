#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;

/// Builds an unsigned-but-well-formed JWT-shaped string (header.payload.sig)
/// carrying the given claims, sufficient for the structural, unverified
/// `parent_agent` read this module performs. Signature verification is out
/// of scope for this SDK.
fn fake_jwt(claims: &serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(serde_json::json!({"alg": "ES256", "typ": "aa-agent+jwt"}).to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    format!("{header}.{payload}.sig")
}

fn root_agent_jwt() -> String {
    fake_jwt(&serde_json::json!({
        "iss": "https://ap.example",
        "sub": "aauth:planner@agent.example",
        "jti": "jti-1",
        "iat": 1_700_000_000,
        "exp": 1_700_003_600,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": {"kty": "EC", "crv": "P-256", "x": "x", "y": "y"}},
    }))
}

fn subagent_jwt_with_parent(parent: &str) -> String {
    fake_jwt(&serde_json::json!({
        "iss": "https://ap.example",
        "sub": "aauth:planner+search1@agent.example",
        "jti": "jti-2",
        "iat": 1_700_000_000,
        "exp": 1_700_003_600,
        "dwk": "aauth-agent.json",
        "cnf": {"jwk": {"kty": "EC", "crv": "P-256", "x": "x2", "y": "y2"}},
        "parent_agent": parent,
    }))
}

#[test]
fn parent_agent_of_reads_claim_when_present() {
    let jwt = subagent_jwt_with_parent("aauth:planner@agent.example");
    assert_eq!(parent_agent_of(&jwt), Some("aauth:planner@agent.example".to_string()));
}

#[test]
fn parent_agent_of_is_none_when_absent() {
    let jwt = root_agent_jwt();
    assert_eq!(parent_agent_of(&jwt), None);
}

#[test]
fn can_mint_subagent_under_allows_root_agent() {
    let jwt = root_agent_jwt();
    assert!(can_mint_subagent_under(&jwt).is_ok());
}

#[test]
fn can_mint_subagent_under_rejects_prospective_parent_that_is_itself_a_subagent() {
    let jwt = subagent_jwt_with_parent("aauth:planner@agent.example");
    let err = can_mint_subagent_under(&jwt).unwrap_err();
    assert!(matches!(err, SubAgentError::ParentAlreadySubAgent));
}

#[test]
fn build_subagent_token_request_rejects_parent_that_is_itself_a_subagent() {
    let parent_jwt = subagent_jwt_with_parent("aauth:root@agent.example");
    let subagent_jwt = subagent_jwt_with_parent("aauth:someone@agent.example");
    let err = build_subagent_token_request(&parent_jwt, "resource-token", &subagent_jwt).unwrap_err();
    assert!(matches!(err, SubAgentError::ParentIsSubAgent));
}

#[test]
fn build_subagent_token_request_rejects_non_subagent_token() {
    let parent_jwt = root_agent_jwt();
    let not_a_subagent_jwt = root_agent_jwt();
    let err = build_subagent_token_request(&parent_jwt, "resource-token", &not_a_subagent_jwt).unwrap_err();
    assert!(matches!(err, SubAgentError::NotASubAgentToken));
}

#[test]
fn build_subagent_token_request_happy_path_sets_fields() {
    let parent_jwt = root_agent_jwt();
    let subagent_jwt = subagent_jwt_with_parent("aauth:planner@agent.example");
    let request = build_subagent_token_request(&parent_jwt, "sub-resource-token", &subagent_jwt)
        .expect("valid parent + subagent should build a request");
    assert_eq!(request.resource_token, "sub-resource-token");
    assert_eq!(request.subagent_token.as_deref(), Some(subagent_jwt.as_str()));
}
