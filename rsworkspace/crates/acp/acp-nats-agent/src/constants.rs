use std::time::Duration;

pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Subject tail of the bridge's `session.ready` signal,
/// `{prefix}.v1.session.{id}.agent.ext.ready` (see `acp_nats::ExtReadySubject`).
pub const SESSION_READY_SUFFIX: &str = ".agent.ext.ready";
