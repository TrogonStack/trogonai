use serde::{Deserialize, Serialize};

use super::TemplatePhase;

/// What the KV index records about one template version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateRecord {
    pub atespace: String,
    pub name: String,
    pub phase: TemplatePhase,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
