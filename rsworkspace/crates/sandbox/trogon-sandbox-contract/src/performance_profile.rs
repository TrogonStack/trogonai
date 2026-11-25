use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PerformanceProfile {
    pub cold_start_p50_ms: u32,
    pub cold_restore_p50_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_sandboxes: Option<u32>,
}
