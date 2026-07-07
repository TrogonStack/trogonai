//! Access Server federation wire types per draft section "Access Server Federation":
//! "AS Token Endpoint" (PS-to-AS token request, AS response, auth token delivery)
//! and "Claims Required". "PS-AS Federation" trust establishment is prose-only
//! (no additional wire shapes beyond the token endpoint and requirement responses
//! already modeled), so it is not separately typed here.

use serde::{Deserialize, Serialize};

/// PS-to-AS token request body, per "PS-to-AS Token Request".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsTokenRequest {
    pub resource_token: String,
    pub agent_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_token: Option<String>,
}

/// AS direct grant response (`200`), per "AS Response". Identical shape to the PS's
/// [`super::person_server::TokenGrantResponse`]; kept as a distinct type since the
/// two endpoints are independently versionable per the draft's role separation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsTokenResponse {
    pub auth_token: String,
    pub expires_in: i64,
}

/// Claims submission POSTed to the `Location` URL in response to
/// `requirement=claims`, per "Claims Required": `{sub, email, tenant, ...}`. The
/// draft shows an open-ended object of claim name to value with `sub` always
/// present as the directed identifier; modeled as a required `sub` plus a
/// free-form map for any other identity claims, since the draft does not enumerate
/// a fixed claim set here (claim names come from the `required_claims` array).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimsSubmission {
    pub sub: String,
    #[serde(flatten)]
    pub claims: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
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
}
