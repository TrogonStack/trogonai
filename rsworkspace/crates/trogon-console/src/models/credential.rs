use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    OAuth,
    BearerToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Active,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialVault {
    pub id: String,
    pub env_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub vault_id: String,
    pub env_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    pub mcp_server_url: String,
    pub status: CredentialStatus,
    pub rotation_policy_days: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCredentialRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    pub mcp_server_url: String,
    pub rotation_policy_days: Option<u32>,
}
