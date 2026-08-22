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

/// Queue depth between a task-events pull loop and the reader of its
/// [`crate::client::TypedEventStream`]. The bound is what turns a slow reader
/// into backpressure: the pull loop waits for room before acking, so the
/// backlog stays in JetStream instead of accumulating in this process.
pub const EVENT_STREAM_QUEUE_CAPACITY: usize = 128;

/// NATS message header carrying the [`crate::ReqId`] across the request/reply boundary.
///
/// Defined once in `trogon-nats` (ADR#0013 / RFC 6648 `Trogon-` prefix).
pub use trogon_nats::REQ_ID_HEADER;

/// Optional correlate external caller identity on gateway ingress (`a2a-gateway` records it on spans /
/// audits). **`a2a-bridge`** should copy HTTP header [`GATEWAY_CALLER_ID_HTTP`] into this NATS header when
/// publishing to `{prefix}.v1.gateway.>`.
///
/// Stable wire token (case-insensitive matching on ingress); prefer `GatewayCallerId` casing in observability prose.
pub const GATEWAY_CALLER_ID_HEADER: &str = "X-A2a-Caller-Id";

/// HTTP correlate for [`GATEWAY_CALLER_ID_HEADER`] on the bridge ingress path (`POST /`).
pub const GATEWAY_CALLER_ID_HTTP: &str = "x-a2a-caller-id";

/// JSON-encoded minted User JWT `data` claim (SpiceDB principal payload) forwarded by gateway
/// ingress onto agent-bound NATS messages. Agent `Bridge` reads this for push DLQ `{caller_id}`.
pub const GATEWAY_PRINCIPAL_HEADER: &str = "X-A2a-Spicedb-Principal";

/// Maximum attempts for terminal task push delivery over HTTPS, core NATS (`subject:` URLs), and JetStream (`jetstream:` URLs).
///
/// HTTPS retries are status- and transport-aware in [`crate::push::HttpPushDispatcher`]. NATS targets use the same
/// attempt cap with exponential backoff in [`crate::push::NatsPublishPushDispatcher`] and
/// [`crate::push::JetStreamPublishPushDispatcher`].
pub const HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS: u32 = 3;

/// Default `{caller_id}` segment for `{prefix}.v1.push.dlq.{caller_id}.{task_id}` when the agent
/// bridge has no authenticated external caller identity (until gateway/auth-callout propagates one).
pub const DEFAULT_PUSH_DLQ_CALLER_SEGMENT: &str = "_";

pub const ENV_PUSH_DLQ_CALLER_SEGMENT: &str = "A2A_PUSH_DLQ_CALLER_SEGMENT";

/// In-process LRU capacity for push DLQ publish dedupe (agent + gateway mirror).
pub const ENV_PUSH_DLQ_DEDUP_LRU_SIZE: &str = "A2A_PUSH_DLQ_DEDUP_LRU_SIZE";

pub const DEFAULT_PUSH_DLQ_DEDUP_LRU_SIZE: usize = 1024;

/// JetStream duplicate suppression window for **`A2A_PUSH_DLQ`** publishes.
pub const ENV_PUSH_DLQ_DEDUP_WINDOW_SECS: &str = "A2A_PUSH_DLQ_DEDUP_WINDOW_SECS";

pub const DEFAULT_PUSH_DLQ_DEDUP_WINDOW_SECS: u64 = 120;

/// JetStream message dedupe header for push DLQ and mirror publishes.
pub const NATS_MSG_ID_HEADER: &str = "Nats-Msg-Id";

pub const ENV_MAX_CONCURRENT_CLIENT_TASKS: &str = "A2A_MAX_CONCURRENT_CLIENT_TASKS";

// -- catalog::nats_kv --

pub const A2A_AGENT_CARDS: &str = "A2A_AGENT_CARDS";

pub(crate) const MAX_VALUE_SIZE: i32 = 65536;

// -- catalog::import_gate::spicedb::config --

#[cfg(feature = "spicedb")]
pub const ENV_SPICEDB_ENDPOINT: &str = "A2A_SPICEDB_ENDPOINT";
#[cfg(feature = "spicedb")]
pub const ENV_SPICEDB_TOKEN: &str = "A2A_SPICEDB_TOKEN";
#[cfg(feature = "spicedb")]
pub const ENV_SPICEDB_ZEDTOKEN_TTL_SECS: &str = "A2A_SPICEDB_ZEDTOKEN_TTL_SECS";

#[cfg(feature = "spicedb")]
pub(crate) const DEFAULT_ZEDTOKEN_TTL_SECS: u64 = 30;

// -- catalog::import_gate::spicedb --

#[cfg(feature = "spicedb")]
pub(crate) const FEDERATED_AGENT_CARD_RESOURCE_TYPE: &str = "agent_card";
#[cfg(feature = "spicedb")]
pub(crate) const FEDERATED_IMPORT_PERMISSION: &str = "view";

// -- catalog::spicedb_permission --

#[cfg(feature = "spicedb")]
pub const ENV_TIER1_SPICEDB_ENABLED: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENABLED";
#[cfg(feature = "spicedb")]
pub const ENV_TIER1_SPICEDB_ENDPOINT: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT";
#[cfg(feature = "spicedb")]
pub const ENV_TIER1_SPICEDB_TOKEN: &str = "A2A_GATEWAY_TIER1_SPICEDB_TOKEN";
#[cfg(feature = "spicedb")]
pub const ENV_TIER1_ZEDTOKEN_TTL_SECS: &str = "A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS";

#[cfg(feature = "spicedb")]
pub(crate) const DEFAULT_TIER1_ZEDTOKEN_TTL_SECS: u64 = 60;
#[cfg(feature = "spicedb")]
pub(crate) const SESSION_CACHE_CAPACITY: u64 = 4096;

// -- error --

/// A2A-defined: requested task ID is unknown to the agent.
pub const TASK_NOT_FOUND: i32 = -32001;

/// A2A-defined: task exists but is not in a cancelable state (already terminal or interrupted).
pub const TASK_NOT_CANCELABLE: i32 = -32002;

/// A2A-defined: agent does not support push notifications.
pub const PUSH_NOTIFICATION_NOT_SUPPORTED: i32 = -32003;

/// A2A-defined: operation not supported by this agent's declared capabilities.
pub const UNSUPPORTED_OPERATION: i32 = -32004;

/// A2A-defined: requested content media type is not supported.
pub const CONTENT_TYPE_NOT_SUPPORTED: i32 = -32005;

/// A2A-defined: agent response is malformed or otherwise invalid.
pub const INVALID_AGENT_RESPONSE: i32 = -32006;

/// A2A-defined: agent does not have an extended AgentCard configured to return.
pub const EXTENDED_AGENT_CARD_NOT_CONFIGURED: i32 = -32007;

/// A2A-defined: client required an extension the agent does not support.
pub const EXTENSION_SUPPORT_REQUIRED: i32 = -32008;

/// A2A-defined: client requested an A2A protocol version this agent cannot serve.
pub const VERSION_NOT_SUPPORTED: i32 = -32009;

/// Binding-specific: no agent replicas are reachable on the agent subject (no responders).
pub const AGENT_UNAVAILABLE: i32 = -32050;

// -- gateway_ingress --

/// Recognized dotted method suffix tokens after `{prefix}.v1.agents.{agent_id}.` /
/// ingress remainder (same spelling as [`crate::server::dispatch::A2aMethod`] mapping).
///
/// Listed longest-first to ensure deterministic matching.
pub const GATEWAY_INGRESS_METHOD_SUFFIXES: &[&[&str]] = &[
    &["message", "stream"],
    &["message", "send"],
    &["tasks", "resubscribe"],
    &["tasks", "cancel"],
    &["tasks", "list"],
    &["tasks", "get"],
    &["push", "set"],
    &["push", "get"],
    &["push", "list"],
    &["push", "delete"],
    &["card"],
];

// -- jetstream::consumers --

pub(crate) const INACTIVE_THRESHOLD: Duration = Duration::from_secs(300);

// -- push::dispatcher (http, jetstream, nats) --

pub(crate) const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
pub(crate) const MAX_RETRY_DELAY: Duration = Duration::from_secs(3);

// -- push::dlq --

pub const PUSH_DLQ_SCHEMA_V1: &str = "a2a.v1.push.dlq/v1";

// -- push::push_idempotency_key --

pub(crate) const TERMINAL_KIND: &str = "terminal";
pub(crate) const DLQ_KIND: &str = "dlq";

// -- push::push_notification_target --

pub(crate) const SUBJECT_SCHEME_PREFIX: &str = "subject:";
pub(crate) const JETSTREAM_SCHEME_PREFIX: &str = "jetstream:";

// -- push::push_payload --

pub const PUSH_IDEMPOTENCY_JSON_FIELD: &str = "_a2aPushIdempotencyKey";

// -- server::* (per-operation JSON-RPC method names) --

pub(crate) const AGENT_CARD_METHOD: &str = "agent/getAuthenticatedExtendedCard";
pub(crate) const MESSAGE_SEND_METHOD: &str = "message/send";
pub(crate) const MESSAGE_STREAM_METHOD: &str = "message/stream";
pub(crate) const PUSH_DELETE_METHOD: &str = "tasks/pushNotificationConfig/delete";
pub(crate) const PUSH_GET_METHOD: &str = "tasks/pushNotificationConfig/get";
pub(crate) const PUSH_LIST_METHOD: &str = "tasks/pushNotificationConfig/list";
pub(crate) const PUSH_SET_METHOD: &str = "tasks/pushNotificationConfig/set";
pub(crate) const TASKS_CANCEL_METHOD: &str = "tasks/cancel";
pub(crate) const TASKS_GET_METHOD: &str = "tasks/get";
pub(crate) const TASKS_LIST_METHOD: &str = "tasks/list";
pub(crate) const TASKS_RESUBSCRIBE_METHOD: &str = "tasks/resubscribe";

#[cfg(test)]
pub const TEST_TASK_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests;
