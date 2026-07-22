use super::*;

#[test]
fn as_token_request_matches_draft_example() {
    let raw = serde_json::json!({
        "resource_token": "eyJhbGc...",
        "agent_token": "eyJhbGc..."
    });
    let req: AsTokenRequest = serde_json::from_value(raw.clone()).unwrap();
    assert!(req.subagent_token.is_none());
    assert_eq!(serde_json::to_value(&req).unwrap(), raw);
}

#[test]
fn as_token_request_with_subagent_and_upstream() {
    let req = AsTokenRequest {
        resource_token: "rt".into(),
        agent_token: "at".into(),
        subagent_token: Some("sat".into()),
        upstream_token: Some("ut".into()),
    };
    let json = serde_json::to_value(&req).unwrap();
    let back: AsTokenRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back, req);
}

#[test]
fn as_token_response_matches_draft_example() {
    let raw = serde_json::json!({"auth_token": "eyJhbGc...", "expires_in": 3600});
    let resp: AsTokenResponse = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(resp.expires_in, 3600);
    assert_eq!(serde_json::to_value(&resp).unwrap(), raw);
}

#[test]
fn claims_submission_matches_trust_establishment_example() {
    let raw = serde_json::json!({"sub": "user:alice", "email": "alice@example.com", "tenant": "corp"});
    let submission: ClaimsSubmission = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(submission.sub, "user:alice");
    assert_eq!(submission.claims.get("email").unwrap(), "alice@example.com");
    assert_eq!(serde_json::to_value(&submission).unwrap(), raw);
}

#[test]
fn claims_submission_with_only_sub() {
    let raw = serde_json::json!({"sub": "user:alice"});
    let submission: ClaimsSubmission = serde_json::from_value(raw.clone()).unwrap();
    assert!(submission.claims.is_empty());
    assert_eq!(serde_json::to_value(&submission).unwrap(), raw);
}
