use std::time::Duration;

pub const ENV_ACP_PREFIX: &str = "ACP_PREFIX";
pub const DEFAULT_ACP_PREFIX: &str = "acp";

pub const ENV_OPERATION_TIMEOUT_SECS: &str = "ACP_OPERATION_TIMEOUT_SECS";
pub const ENV_PROMPT_TIMEOUT_SECS: &str = "ACP_PROMPT_TIMEOUT_SECS";
pub const ENV_CONNECT_TIMEOUT_SECS: &str = "ACP_NATS_CONNECT_TIMEOUT_SECS";

pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(7200);
pub const DEFAULT_MAX_CONCURRENT_CLIENT_TASKS: usize = 256;
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const MIN_TIMEOUT_SECS: u64 = 1;

// TODO: replace with Duration::from_days(30) once duration_constructors is stable
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_hours(30 * 24);
pub const SESSION_READY_DELAY: Duration = Duration::from_millis(100);
pub const PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW: Duration = Duration::from_secs(5);
pub const TEST_PROMPT_TIMEOUT: Duration = Duration::from_secs(5);

pub const AGENT_UNAVAILABLE: i32 = -32001;

pub const SESSION_PREFIX: &str = ".session.";
pub const SESSION_AGENT_MARKER: &str = ".agent.";
pub const SESSION_CLIENT_MARKER: &str = ".client.";
pub const EXT_SUBJECT_PREFIX: &str = "ext.";

pub const CONTENT_TYPE_JSON: &str = "application/json";
pub const CONTENT_TYPE_PLAIN: &str = "text/plain";

pub const SESSION_ID_HEADER: &str = "X-Session-Id";
pub const CAUSATION_ID_HEADER: &str = "X-Causation-Id";
