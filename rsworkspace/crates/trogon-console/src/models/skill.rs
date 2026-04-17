use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub latest_version: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillVersion {
    pub skill_id: String,
    pub version: String,
    pub content: String,
    pub is_latest: bool,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub description: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    pub content: String,
}

fn default_provider() -> String {
    "custom".to_string()
}

#[derive(Debug, Deserialize)]
pub struct CreateSkillVersionRequest {
    pub content: String,
}
