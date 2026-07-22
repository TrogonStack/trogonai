use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModel {
    pub id: String,
    #[serde(default = "default_speed")]
    pub speed: String,
}

fn default_speed() -> String {
    "standard".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: AgentStatus,
    pub version: u32,
    pub model: AgentModel,
    pub system_prompt: String,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: String,
    pub model: AgentModel,
    pub system_prompt: String,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<AgentStatus>,
    pub model: Option<AgentModel>,
    pub system_prompt: Option<String>,
    pub skill_ids: Option<Vec<String>>,
    pub tools: Option<Vec<serde_json::Value>>,
    pub mcp_servers: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentVersion {
    pub version: u32,
    pub updated_at: String,
    pub model_id: String,
}
