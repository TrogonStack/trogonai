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

/// Per-Account override for [`DEFAULT_STREAM_MAX_AGE`] on **`A2A_EVENTS`** provisioning.
pub const ENV_EVENTS_MAX_AGE_SECS: &str = "A2A_EVENTS_MAX_AGE_SECS";

/// Default timeout for unary methods (message/send unary, tasks/get, etc).
pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for streaming task completion (message/stream end-to-terminal-state).
/// Two hours mirrors `acp-nats::DEFAULT_PROMPT_TIMEOUT`; long-running agent work is normal.
pub const DEFAULT_TASK_TIMEOUT: Duration = Duration::from_secs(7200);

/// Default per-client back-pressure cap on concurrent streaming tasks.
pub const DEFAULT_MAX_CONCURRENT_CLIENT_TASKS: usize = 256;

/// NATS message header carrying the [`crate::ReqId`] across the request/reply boundary.
pub const REQ_ID_HEADER: &str = "X-Req-Id";

/// Optional correlate external caller identity on gateway ingress (`a2a-gateway` records it on spans /
/// audits). **`a2a-bridge`** should copy HTTP header [`GATEWAY_CALLER_ID_HTTP`] into this NATS header when
/// publishing to `{prefix}.gateway.>`.
///
/// Stable wire token (case-insensitive matching on ingress); prefer `GatewayCallerId` casing in observability prose.
pub const GATEWAY_CALLER_ID_HEADER: &str = "X-A2a-Caller-Id";

/// HTTP correlate for [`GATEWAY_CALLER_ID_HEADER`] on the bridge ingress path (`POST /`).
pub const GATEWAY_CALLER_ID_HTTP: &str = "x-a2a-caller-id";

/// Maximum attempts for terminal task push delivery over HTTPS, core NATS (`subject:` URLs), and JetStream (`jetstream:` URLs).
///
/// HTTPS retries are status- and transport-aware in [`crate::push::HttpPushDispatcher`]. NATS targets use the same
/// attempt cap with exponential backoff in [`crate::push::NatsPublishPushDispatcher`] and
/// [`crate::push::JetStreamPublishPushDispatcher`].
pub const HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS: u32 = 3;

/// Default `{caller_id}` segment for `{prefix}.push.dlq.{caller_id}.{task_id}` when the agent
/// bridge has no authenticated external caller identity (until gateway/auth-callout propagates one).
pub const DEFAULT_PUSH_DLQ_CALLER_SEGMENT: &str = "_";

pub const ENV_PUSH_DLQ_CALLER_SEGMENT: &str = "A2A_PUSH_DLQ_CALLER_SEGMENT";

pub const ENV_MAX_CONCURRENT_CLIENT_TASKS: &str = "A2A_MAX_CONCURRENT_CLIENT_TASKS";

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
            ENV_PUSH_DLQ_CALLER_SEGMENT,
            ENV_MAX_CONCURRENT_CLIENT_TASKS,
            ENV_EVENTS_MAX_AGE_SECS,
        ] {
            assert!(name.starts_with("A2A_"), "{name} not A2A-namespaced");
        }
    }

    #[test]
    fn headers_use_x_prefix() {
        assert!(REQ_ID_HEADER.starts_with("X-"));
        assert!(GATEWAY_CALLER_ID_HEADER.starts_with("X-"));
        assert!(
            GATEWAY_CALLER_ID_HTTP
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "HTTP header literals should be lowercase with digits/hyphen only for axum pairing"
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn http_push_max_attempts_is_positive() {
        assert!(crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS > 0);
    }
}
