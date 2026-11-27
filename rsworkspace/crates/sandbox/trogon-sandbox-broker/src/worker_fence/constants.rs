use std::time::Duration;

pub const WORKER_FENCE_BUCKET: &str = "sandbox-worker-fences";

/// How often a broker re-reads the fence bucket.
pub const FENCE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
