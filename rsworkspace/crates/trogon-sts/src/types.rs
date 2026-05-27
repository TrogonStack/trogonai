use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StsExchangeRequest {
    pub subject_token: String,
    pub subject_token_type: String,
    pub actor_token: String,
    pub audience: String,
    pub scope: String,
    pub purpose: String,
    pub requested_token_type: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StsExchangeResponse {
    pub access_token: String,
    pub issued_token_type: String,
    #[serde(rename = "token_type")]
    pub token_type_bearer: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StsTokenErrorResponse {
    pub error: String,
    pub error_description: String,
}

impl StsTokenErrorResponse {
    pub fn from_sts_error(err: &crate::error::StsError) -> Self {
        Self {
            error: err.error_code().to_string(),
            error_description: err.error_description(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct VerifiedSubjectClaims {
    pub sub: String,
    pub iss: String,
    pub aud: serde_json::Value,
    pub exp: i64,
    pub iat: i64,
    pub agent_id: Option<String>,
    pub agent_version: Option<String>,
    pub wkl: Option<String>,
    pub purpose: Option<String>,
    pub scope: Option<String>,
    pub tenant: Option<String>,
    pub act_chain: Vec<trogon_identity_types::ActChainEntry>,
    pub raw: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MintedClaimsSummary {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: i64,
    pub iat: i64,
    pub agent_id: Option<String>,
    pub wkl: Option<String>,
    pub purpose: Option<String>,
    pub scope: String,
    pub act_chain_depth: usize,
}
