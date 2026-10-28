//! Crate-wide constants for the A2A gateway: env-var names, defaults,
//! wire tokens, and tuning values used across the ingress/egress pumps,
//! AAuth layer, and policy tiers.

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

// --- aauth.rs ---

/// Anti-replay code returned on enforce-mode AAuth failure. JSON-RPC clients
/// see this code; A2A ingress callers see a status reply carrying the
/// `AAuth-Requirement` header.
pub const AAUTH_REQUIRED_CODE: i32 = -32_118;

// --- jwt_caller_identity.rs ---

pub const ENV_GATEWAY_TRUST_CALLER_HEADERS: &str = "A2A_GATEWAY_TRUST_CALLER_HEADERS";
pub const ENV_GATEWAY_JWT_AUDIENCE: &str = "A2A_GATEWAY_JWT_AUDIENCE";

// --- runtime/tier1.rs ---

/// Sentinel slug used when an ingress request carries no caller
/// identity. Distinct from any real caller value so a deny-anonymous
/// declarative rule can match on it precisely. Kept as a single
/// constant so the value matches between the context builder and
/// the bundle authors who write rules against it.
#[cfg(feature = "spicedb")]
pub const ANONYMOUS_CALLER_SLUG: &str = "_";

// --- runtime/dispatch.rs ---
// The anonymous-caller sentinel and JWT-audience env var are single-sourced
// as `ANONYMOUS_CALLER_SLUG` and `ENV_GATEWAY_JWT_AUDIENCE` above; dispatch
// references those rather than redeclaring the literals.

#[cfg(feature = "spicedb")]
pub(crate) const SPAN_GATEWAY_INGRESS_DISPATCH: &str = "gateway.ingress.dispatch";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_CALLER_ID: &str = "caller_id";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_AGENT_SUBJECT: &str = "agent_subject";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_ROUTING_OUTCOME: &str = "routing_outcome";
#[cfg(feature = "spicedb")]
pub(crate) const ATTR_AAUTH_AGENT_ID: &str = "aauth_agent_id";

#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_IGNORED_NO_REPLY: &str = "ignored_no_reply";
#[cfg(feature = "spicedb")]
pub(crate) const ROUTING_AAUTH_DENIED: &str = "aauth_denied";
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

// --- runtime/aauth_env.rs ---

pub const ENV_AAUTH_MODE: &str = "A2A_GATEWAY_AAUTH_MODE";
pub const ENV_AAUTH_JWKS_PATH: &str = "A2A_GATEWAY_AAUTH_JWKS_PATH";
pub const ENV_AAUTH_JWKS_DISCOVERY: &str = "A2A_GATEWAY_AAUTH_JWKS_DISCOVERY";
pub const ENV_AAUTH_JWKS_TTL_SECS: &str = "A2A_GATEWAY_AAUTH_JWKS_TTL_SECS";
pub const ENV_AAUTH_JWKS_ALLOWED_ISSUERS: &str = "A2A_GATEWAY_AAUTH_JWKS_ALLOWED_ISSUERS";
pub const ENV_AAUTH_RESOURCE_ISS: &str = "A2A_GATEWAY_AAUTH_RESOURCE_ISS";
pub const ENV_AAUTH_PERSON_SERVER_AUD: &str = "A2A_GATEWAY_AAUTH_PERSON_SERVER_AUD";
pub const ENV_AAUTH_CHALLENGE_KID: &str = "A2A_GATEWAY_AAUTH_CHALLENGE_KID";
pub const ENV_AAUTH_CHALLENGE_KEY_PATH: &str = "A2A_GATEWAY_AAUTH_CHALLENGE_KEY_PATH";
pub const ENV_AAUTH_LEEWAY_SECS: &str = "A2A_GATEWAY_AAUTH_LEEWAY_SECS";
pub const ENV_AAUTH_CHALLENGE_TTL_SECS: &str = "A2A_GATEWAY_AAUTH_CHALLENGE_TTL_SECS";
pub const ENV_AAUTH_MAX_SKEW_SECS: &str = "A2A_GATEWAY_AAUTH_MAX_SKEW_SECS";

/// Audit `caller_source` recorded once a verified `aa-auth+jwt` principal
/// supersedes the JWT-header caller identity for the remainder of dispatch.
pub const AAUTH_CALLER_SOURCE: &str = "aauth";

pub(crate) const DEFAULT_LEEWAY_SECS: u64 = 60;
pub(crate) const DEFAULT_CHALLENGE_TTL_SECS: i64 = 300;
pub(crate) const DEFAULT_MAX_SKEW_SECS: i64 = 60;

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
