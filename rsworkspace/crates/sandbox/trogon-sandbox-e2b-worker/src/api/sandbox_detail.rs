use serde::Deserialize;

/// `GET /sandboxes/{id}`: the fields the worker reads.
#[derive(Debug, Clone, Deserialize)]
pub struct SandboxDetail {
    #[serde(default)]
    pub state: String,
    #[serde(rename = "cpuCount", default)]
    pub cpu_count: Option<u32>,
    #[serde(rename = "memoryMB", default)]
    pub memory_mb: Option<u32>,
}
