//! Crate-wide constants for the A2A gateway: env-var names, defaults,
//! wire tokens, and tuning values used across the ingress/egress pumps
//! and policy tiers.

use std::time::Duration;

// --- config.rs ---

pub(crate) const ENV_GATEWAY_QUEUE_GROUP: &str = "A2A_GATEWAY_QUEUE_GROUP";

// --- gw_ingress_stream.rs ---

pub const ENV_GATEWAY_STREAMING_INGRESS: &str = "A2A_GATEWAY_STREAMING_INGRESS";
pub const ENV_GATEWAY_STREAMING_MAX_ACK_PENDING: &str = "A2A_GATEWAY_STREAMING_MAX_ACK_PENDING";
pub const ENV_GATEWAY_STREAMING_MAX_INFLIGHT: &str = "A2A_GATEWAY_STREAMING_MAX_INFLIGHT";

pub const DEFAULT_STREAMING_MAX_ACK_PENDING: i64 = 32;
pub const DEFAULT_STREAMING_MAX_INFLIGHT: usize = 32;

/// Cap on JetStream redelivery attempts before the streaming ingress pump
/// Term's a message that the caller reply persistently rejects. Mirrors the
/// 3-attempt budget the egress planner uses so the two pumps behave the
/// same under a permanently broken reply subject (bad ACL, closed inbox).
pub(crate) const STREAMING_INGRESS_MAX_FORWARD_ATTEMPTS: i64 = 3;

// --- gw_pull_backpressure.rs ---

pub const ENV_GATEWAY_EVENTS_PULL: &str = "A2A_GATEWAY_EVENTS_PULL";
pub const ENV_GATEWAY_EVENTS_MAX_ACK_PENDING: &str = "A2A_GATEWAY_EVENTS_MAX_ACK_PENDING";
pub const ENV_GATEWAY_EVENTS_FETCH_BATCH: &str = "A2A_GATEWAY_EVENTS_FETCH_BATCH";
pub const ENV_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS: &str = "A2A_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS";
pub const ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER: &str = "A2A_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER";

pub const DEFAULT_MAX_ACK_PENDING: usize = 1024;
pub const DEFAULT_FETCH_BATCH: usize = 1;
pub const DEFAULT_FETCH_HEARTBEAT_SECS: u64 = 5;
pub const DEFAULT_INACTIVE_THRESHOLD_SECS: u64 = 300;
pub const DEFAULT_MAX_INFLIGHT_PER_CALLER: usize = 32;

pub(crate) const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(30);
pub(crate) const FETCH_EXPIRES: Duration = Duration::from_secs(30);
/// Delay applied when the per-caller inflight gate is full. Keeps the
/// JetStream redelivery rate bounded while the offending caller's other
/// in-flight forwards drain — without a delay JetStream would re-deliver
/// immediately and the pump would burn CPU on rejected messages.
pub(crate) const GATE_NAK_DELAY: Duration = Duration::from_millis(500);

// --- push_dlq_mirror.rs ---

pub const ENV_PUSH_DLQ_MIRROR: &str = "A2A_GATEWAY_PUSH_DLQ_MIRROR";
pub const ENV_PUSH_DLQ_MIRROR_DURABLE: &str = "A2A_GATEWAY_PUSH_DLQ_DURABLE";
pub const PUSH_DLQ_MIRROR_HEADER: &str = "X-A2a-Dlq-Mirrored";
/// Prefix applied to the `Nats-Msg-Id` header on mirror publishes so the
/// authoritative DLQ envelope and its mirror don't collide in JetStream's
/// `duplicate_window` dedup — they share a stream.
pub const MIRROR_MSG_ID_PREFIX: &str = "mirror:";

pub(crate) const MAX_PUBLISH_RETRIES: u32 = 3;
pub(crate) const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

// --- ingress_identity.rs ---

pub const ENV_GATEWAY_JWT_AUDIENCE: &str = "A2A_GATEWAY_JWT_AUDIENCE";

// --- runtime/dispatch.rs ---

#[cfg(feature = "spicedb")]
pub(crate) const SPAN_GATEWAY_INGRESS_DISPATCH: &str = "gateway.ingress.dispatch";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_CALLER_ID: &str = "caller_id";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_AGENT_SUBJECT: &str = "agent_subject";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_ROUTING_OUTCOME: &str = "routing_outcome";

#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_IGNORED_NO_REPLY: &str = "ignored_no_reply";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_TIER1_DENIED: &str = "tier1_denied";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_POLICY_DENIED: &str = "policy_denied";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_TIER3_REFUSED: &str = "tier3_refused";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_TIER3_ENGINE_ERROR: &str = "tier3_engine_error";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_FORWARDED: &str = "forwarded";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_FORWARD_FAILED: &str = "forward_failed";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_DEADLINE_EXCEEDED: &str = "deadline_exceeded";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_INGRESS_ERROR: &str = "ingress_error";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_CALLER_JWT_REJECTED: &str = "caller_jwt_rejected";

// --- runtime/env.rs ---

pub const ENV_GATEWAY_TIER2_CEL_ENABLED: &str = "A2A_GATEWAY_TIER2_CEL_ENABLED";
pub const ENV_GATEWAY_TIER3_SIGNING_PUBKEY: &str = "A2A_GATEWAY_TIER3_SIGNING_PUBKEY";
pub const ENV_GATEWAY_AUDIT_PUBLISH: &str = "A2A_GATEWAY_AUDIT_PUBLISH";
pub const ENV_GATEWAY_UNARY_DEADLINE_SECS: &str = "A2A_GATEWAY_UNARY_DEADLINE_SECS";

/// Method-dots string for the unary `message/send`. Shared by
/// `runtime::env` (unary deadline lookup) and `runtime::dispatch`
/// (`spicedb` builds).
pub(crate) const MESSAGE_SEND_METHOD_DOTS: &str = "message.send";

// --- runtime/streaming.rs ---

/// Method-dots string for the unary version of `message/send`.
pub(crate) const MESSAGE_STREAM_METHOD_DOTS: &str = "message.stream";
pub(crate) const TASKS_RESUBSCRIBE_METHOD_DOTS: &str = "tasks.resubscribe";

// --- runtime/policy_stack.rs ---

pub(crate) const ENV_POLICY_BUNDLE_DIR: &str = "A2A_GATEWAY_POLICY_BUNDLE_DIR";
pub(crate) const ENV_POLICY_SKILLS: &str = "A2A_GATEWAY_POLICY_SKILLS";
pub(crate) const ENV_TIER3_REDACTION_ENABLED: &str = "A2A_GATEWAY_TIER3_REDACTION_ENABLED";

// --- policy/tier1_declarative/evaluator.rs ---

pub const ENV_TIER1_DECLARATIVE_ENABLED: &str = "A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED";
pub const ENV_TIER1_BUNDLE_DIR: &str = "A2A_GATEWAY_TIER1_BUNDLE_DIR";

// --- policy/tier1_declarative/loader.rs ---

pub const TIER1_BUNDLE_EXTENSION: &str = "tier1.toml";

// --- policy/spicedb_tier1.rs ---

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
