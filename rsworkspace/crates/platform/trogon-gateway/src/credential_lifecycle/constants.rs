use std::time::Duration;

pub(super) const CHECKPOINT_KEY: &str = "v1.recovery-worker";
pub(super) const DEFAULT_INITIAL_FAILURE_BACKOFF: Duration = Duration::from_secs(30);
pub(super) const DEFAULT_MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(15 * 60);
pub(super) const DEFAULT_STUCK_AFTER: Duration = Duration::from_secs(30 * 60);
pub(super) const CREDENTIAL_WORKER_CHECKPOINT_BUCKET: &str = "GATEWAY_CREDENTIAL_WORKER_CHECKPOINTS";
