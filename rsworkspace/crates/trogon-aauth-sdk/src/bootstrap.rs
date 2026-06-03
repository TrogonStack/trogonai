//! Wire types for the Person Server endpoints. Mirrors `trogon-aauth-person::core`
//! intentionally — duplicated rather than re-exported so the SDK has no
//! reverse dependency on the server crate.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapRequest {
    pub cnf_jwk: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub agent_jwt: String,
    pub agent_id: String,
    pub expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRequest {
    pub resource_jwt: String,
    pub agent_jwt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub auth_jwt: String,
    pub scope: String,
    pub expires_in: i64,
}
