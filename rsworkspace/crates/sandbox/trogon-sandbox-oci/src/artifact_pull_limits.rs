use std::time::Duration;

/// Byte, count, and time budgets for [`OciClient::pull_artifact_bounded`](crate::OciClient::pull_artifact_bounded).
#[derive(Debug, Clone, Copy)]
pub struct ArtifactPullLimits {
    pub max_layers: usize,
    pub max_layer_bytes: u64,
    pub max_total_bytes: u64,
    pub timeout: Duration,
}
