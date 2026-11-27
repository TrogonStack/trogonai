use serde::Serialize;

/// Body of a toolbox `process/execute` call.
#[derive(Debug, Clone, Serialize)]
pub struct ExecuteRequest {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Seconds the sandbox gives the command before it ends it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}
