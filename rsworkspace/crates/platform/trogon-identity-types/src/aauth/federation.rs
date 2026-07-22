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
mod tests;
