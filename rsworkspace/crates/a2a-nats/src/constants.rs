//! Constants for the a2a-nats binding.

use std::time::Duration;

pub const DEFAULT_A2A_PREFIX: &str = "a2a";
pub const ENV_A2A_PREFIX: &str = "A2A_PREFIX";

pub const ENV_OPERATION_TIMEOUT_SECS: &str = "A2A_OPERATION_TIMEOUT_SECS";
pub const ENV_TASK_TIMEOUT_SECS: &str = "A2A_TASK_TIMEOUT_SECS";
pub const ENV_CONNECT_TIMEOUT_SECS: &str = "A2A_CONNECT_TIMEOUT_SECS";

pub const MIN_TIMEOUT_SECS: u64 = 1;
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// JetStream retention window for task events (replay budget for `tasks/resubscribe`).
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_hours(24);

/// Default timeout for unary methods (message/send unary, tasks/get, etc).
pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for streaming task completion (message/stream end-to-terminal-state).
/// Two hours mirrors `acp-nats::DEFAULT_PROMPT_TIMEOUT`; long-running agent work is normal.
pub const DEFAULT_TASK_TIMEOUT: Duration = Duration::from_secs(7200);

/// Default per-client back-pressure cap on concurrent streaming tasks.
pub const DEFAULT_MAX_CONCURRENT_CLIENT_TASKS: usize = 256;

/// NATS message header carrying the [`crate::ReqId`] across the request/reply boundary.
pub const REQ_ID_HEADER: &str = "X-Req-Id";

/// NATS message header carrying the resubscribe start sequence for `tasks/resubscribe`.
pub const RESUBSCRIBE_START_SEQ_HEADER: &str = "X-Resubscribe-Start-Seq";

#[cfg(test)]
pub const TEST_TASK_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefix_is_a2a() {
        assert_eq!(DEFAULT_A2A_PREFIX, "a2a");
    }

    #[test]
    fn default_timeouts_are_positive() {
        assert!(DEFAULT_OPERATION_TIMEOUT > Duration::ZERO);
        assert!(DEFAULT_TASK_TIMEOUT > Duration::ZERO);
        const _: () = assert!(DEFAULT_CONNECT_TIMEOUT_SECS > 0);
    }

    #[test]
    fn task_timeout_exceeds_operation_timeout() {
        assert!(DEFAULT_TASK_TIMEOUT > DEFAULT_OPERATION_TIMEOUT);
    }

    #[test]
    fn env_var_names_are_namespaced() {
        for name in [
            ENV_A2A_PREFIX,
            ENV_OPERATION_TIMEOUT_SECS,
            ENV_TASK_TIMEOUT_SECS,
            ENV_CONNECT_TIMEOUT_SECS,
        ] {
            assert!(name.starts_with("A2A_"), "{name} not A2A-namespaced");
        }
    }

    #[test]
    fn headers_use_x_prefix() {
        assert!(REQ_ID_HEADER.starts_with("X-"));
        assert!(RESUBSCRIBE_START_SEQ_HEADER.starts_with("X-"));
    }
}
