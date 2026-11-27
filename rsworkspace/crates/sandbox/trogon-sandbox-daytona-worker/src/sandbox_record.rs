use serde::Deserialize;

use crate::SandboxState;

/// The part of a Daytona sandbox the worker reads. Every field has a default, so a sparse answer still parses.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub state: Option<SandboxState>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub error_reason: Option<String>,
}
