use serde::{Deserialize, Serialize};

pub const TOKEN_TYPE_JWT: &str = "urn:ietf:params:oauth:token-type:jwt";
pub const GRANT_TYPE_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

fn default_actor_token_type() -> String {
    TOKEN_TYPE_JWT.to_string()
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum ExchangeMode {
    #[default]
    Delegation,
    ExchangeOnly,
    ElicitationOnly,
    AuthOnly,
}

impl ExchangeMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "" => Ok(Self::default()),
            "Delegation" | "delegation" => Ok(Self::Delegation),
            "ExchangeOnly" | "exchange-only" | "exchange_only" | "impersonation" | "Impersonation" => {
                Ok(Self::ExchangeOnly)
            }
            "ElicitationOnly" | "elicitation-only" | "elicitation_only" => Ok(Self::ElicitationOnly),
            "AuthOnly" | "auth-only" | "auth_only" => Ok(Self::AuthOnly),
            other => Err(format!("unsupported exchange mode: {other}")),
        }
    }

    pub fn suppresses_act_claim(self) -> bool {
        matches!(self, Self::ExchangeOnly | Self::ElicitationOnly | Self::AuthOnly)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StsExchangeRequest {
    pub subject_token: String,
    pub subject_token_type: String,
    pub actor_token: String,
    #[serde(default = "default_actor_token_type")]
    pub actor_token_type: String,
    pub audience: String,
    pub scope: String,
    pub purpose: String,
    pub requested_token_type: String,
    #[serde(default)]
    pub mode: ExchangeMode,
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
