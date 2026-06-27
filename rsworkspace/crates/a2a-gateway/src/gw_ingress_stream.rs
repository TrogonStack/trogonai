//! Gateway-owned JetStream pull pipe for streaming ingress
//! (`message/stream`, `tasks/resubscribe`).
//!
//! Operator guide:
//! [`../../../../docs/a2a/how-to/operators/streaming-backpressure.md`](../../../../docs/a2a/how-to/operators/streaming-backpressure.md).

use std::collections::HashMap;
use std::sync::Mutex;

use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use tokio_util::sync::CancellationToken;
use trogon_std::env::ReadEnv;

// Pump-only imports — gated so `cfg(coverage)` doesn't warn on unused
// imports when the pump stubs out.
#[cfg(not(coverage))]
use a2a_nats::jetstream::consumers::{gateway_stream_events_consumer, resubscribe_consumer_with_flow};
#[cfg(not(coverage))]
use a2a_nats::jetstream::streams::events_stream_name;
#[cfg(not(coverage))]
use async_nats::jetstream::{self, AckKind};
#[cfg(not(coverage))]
use futures::StreamExt;
#[cfg(not(coverage))]
use tracing::{debug, warn};

pub const ENV_GATEWAY_STREAMING_INGRESS: &str = "A2A_GATEWAY_STREAMING_INGRESS";
pub const ENV_GATEWAY_STREAMING_MAX_ACK_PENDING: &str = "A2A_GATEWAY_STREAMING_MAX_ACK_PENDING";
pub const ENV_GATEWAY_STREAMING_MAX_INFLIGHT: &str = "A2A_GATEWAY_STREAMING_MAX_INFLIGHT";

pub const DEFAULT_STREAMING_MAX_ACK_PENDING: i64 = 32;
pub const DEFAULT_STREAMING_MAX_INFLIGHT: usize = 32;

/// Cap on JetStream redelivery attempts before the streaming ingress pump
/// Term's a message that the caller reply persistently rejects. Mirrors the
/// 3-attempt budget the egress planner uses so the two pumps behave the
/// same under a permanently broken reply subject (bad ACL, closed inbox).
#[cfg(not(coverage))]
const STREAMING_INGRESS_MAX_FORWARD_ATTEMPTS: i64 = 3;

/// JetStream max-ack-pending for a streaming pump. Floored at 1 so a config
/// can't construct a value that stalls the consumer with zero unacked slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamingMaxAckPending(i64);

impl StreamingMaxAckPending {
    #[must_use]
    pub fn new(value: i64) -> Self {
        Self(value.max(1))
    }

    pub fn as_i64(self) -> i64 {
        self.0
    }
}

/// Per-caller concurrent stream cap. Floored at 1 so the inflight gate
/// always lets at least one request through per caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamingMaxInflightPerCaller(usize);

impl StreamingMaxInflightPerCaller {
    #[must_use]
    pub fn new(value: usize) -> Self {
        Self(value.max(1))
    }

    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// Validated identifier for the caller bucket in the per-caller inflight
/// gate. Construction trims and refuses empty values so a configuration
/// bug can't widen every caller into a single "" bucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallerKey(String);

impl CallerKey {
    pub fn new(raw: impl Into<String>) -> Result<Self, CallerKeyError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(CallerKeyError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CallerKeyError {
    #[error("caller key must not be empty")]
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayStreamingIngressConfig {
    max_ack_pending: StreamingMaxAckPending,
    max_inflight_per_caller: StreamingMaxInflightPerCaller,
}

impl GatewayStreamingIngressConfig {
    /// Construct from validated value objects. Used by tests and callers
    /// that already hold typed configs.
    #[must_use]
    pub fn new(
        max_ack_pending: StreamingMaxAckPending,
        max_inflight_per_caller: StreamingMaxInflightPerCaller,
    ) -> Self {
        Self {
            max_ack_pending,
            max_inflight_per_caller,
        }
    }

    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let max_ack_pending = env
            .var(ENV_GATEWAY_STREAMING_MAX_ACK_PENDING)
            .ok()
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .unwrap_or(DEFAULT_STREAMING_MAX_ACK_PENDING);
        let max_inflight_per_caller = env
            .var(ENV_GATEWAY_STREAMING_MAX_INFLIGHT)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_STREAMING_MAX_INFLIGHT);
        Self {
            max_ack_pending: StreamingMaxAckPending::new(max_ack_pending),
            max_inflight_per_caller: StreamingMaxInflightPerCaller::new(max_inflight_per_caller),
        }
    }

    pub fn max_ack_pending(&self) -> StreamingMaxAckPending {
        self.max_ack_pending
    }

    pub fn max_inflight_per_caller(&self) -> StreamingMaxInflightPerCaller {
        self.max_inflight_per_caller
    }
}

pub fn gateway_streaming_ingress_enabled<E: ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var(ENV_GATEWAY_STREAMING_INGRESS) else {
        return false;
    };
    matches!(flag.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// What kind of stream this spawn is — encodes the required-field shape so
/// `TasksResubscribe` can't be represented without a `task_id` (the
/// previous flat struct allowed `task_id: None` which the pump silently
/// dropped at runtime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingIngressKind {
    MessageStream {
        req_id: ReqId,
    },
    TasksResubscribe {
        req_id: ReqId,
        task_id: A2aTaskId,
        last_seq: u64,
    },
}

pub struct StreamingIngressSpawn {
    pub kind: StreamingIngressKind,
    pub reply: async_nats::Subject,
    pub caller_key: CallerKey,
}

#[cfg(not(coverage))]
impl StreamingIngressSpawn {
    fn req_id(&self) -> &ReqId {
        match &self.kind {
            StreamingIngressKind::MessageStream { req_id } | StreamingIngressKind::TasksResubscribe { req_id, .. } => {
                req_id
            }
        }
    }

    fn method_label(&self) -> &'static str {
        match &self.kind {
            StreamingIngressKind::MessageStream { .. } => "message_stream",
            StreamingIngressKind::TasksResubscribe { .. } => "tasks_resubscribe",
        }
    }
}

/// Shared per-caller inflight cap for all streaming-ingress pumps spawned
/// against the same gateway. The runtime constructs one instance and hands
/// it to every `spawn_streaming_ingress_pump` call — otherwise each pump
/// would build a fresh gate and `max_inflight_per_caller` would never
/// constrain concurrent pumps for the same `caller_key`.
#[derive(Clone)]
pub struct StreamingIngressGate {
    inner: std::sync::Arc<CallerInflightGate>,
}

impl StreamingIngressGate {
    /// Build a gate with the configured per-caller limit. Pass the same
    /// instance (clone the cheap `Arc`) to every spawned pump.
    #[must_use]
    pub fn new(config: GatewayStreamingIngressConfig) -> Self {
        Self {
            inner: std::sync::Arc::new(CallerInflightGate::new(config.max_inflight_per_caller.as_usize())),
        }
    }

    fn try_acquire(&self, caller_key: &CallerKey) -> Option<CallerInflightPermit> {
        self.inner.clone().try_acquire(caller_key)
    }
}

struct CallerInflightGate {
    limit: usize,
    inflight: Mutex<HashMap<String, usize>>,
}

impl CallerInflightGate {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// Take a permit. Returns `None` once the per-caller bucket has reached
    /// the configured limit. The returned permit holds an `Arc` back to the
    /// gate so it can be moved into a spawned task and released on drop.
    fn try_acquire(self: std::sync::Arc<Self>, caller_key: &CallerKey) -> Option<CallerInflightPermit> {
        // Recover from a poisoned lock — the inflight map is plain counters;
        // a partial inc/dec can't leave it logically corrupt, and panicking
        // every caller after an unrelated panic would turn the gate into a
        // service-wide DoS.
        let key = caller_key.as_str().to_owned();
        let mut guard = match self.inflight.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let count = guard.entry(key.clone()).or_insert(0);
        if *count >= self.limit {
            return None;
        }
        *count += 1;
        drop(guard);
        Some(CallerInflightPermit {
            gate: self,
            caller_key: key,
        })
    }
}

pub struct CallerInflightPermit {
    gate: std::sync::Arc<CallerInflightGate>,
    caller_key: String,
}

impl Drop for CallerInflightPermit {
    fn drop(&mut self) {
        let mut guard = match self.gate.inflight.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(count) = guard.get_mut(&self.caller_key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                guard.remove(&self.caller_key);
            }
        }
    }
}

/// Spawn-time failure surfaced to the request path so backpressure is
/// observable instead of swallowed inside a detached task.
#[derive(Debug, thiserror::Error)]
pub enum StreamingIngressSpawnError {
    /// The shared gate refused this spawn because the caller is already at
    /// `max_inflight_per_caller`. The request path should map this to a
    /// 429-style response to the caller.
    #[error("caller {caller:?} is at the per-caller inflight limit")]
    PerCallerLimit { caller: String },
}

/// Spawn the streaming-ingress pump on a background task.
///
/// Acquires the per-caller permit BEFORE the detached task so the request
/// path can observe backpressure synchronously — otherwise the caller
/// couldn't tell a successful spawn from one that the gate refused inside
/// the spawned task. The permit moves into the task and releases on drop.
pub fn spawn_streaming_ingress_pump(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayStreamingIngressConfig,
    gate: StreamingIngressGate,
    spawn: StreamingIngressSpawn,
    shutdown: CancellationToken,
) -> Result<(), StreamingIngressSpawnError> {
    let permit = gate
        .try_acquire(&spawn.caller_key)
        .ok_or_else(|| StreamingIngressSpawnError::PerCallerLimit {
            caller: spawn.caller_key.as_str().to_owned(),
        })?;
    #[cfg(not(coverage))]
    tokio::spawn(async move {
        run_streaming_ingress_pump(client, prefix, config, permit, spawn, shutdown).await;
    });
    #[cfg(coverage)]
    drop((client, prefix, config, permit, spawn, shutdown));
    Ok(())
}

#[cfg(not(coverage))]
async fn run_streaming_ingress_pump(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayStreamingIngressConfig,
    _permit: CallerInflightPermit,
    spawn: StreamingIngressSpawn,
    shutdown: CancellationToken,
) {
    let jetstream = jetstream::new(client.clone());
    let stream_name = events_stream_name(&prefix);
    let stream = match jetstream.get_stream(&stream_name).await {
        Ok(stream) => stream,
        Err(error) => {
            warn!(
                stream = %stream_name,
                error = %error,
                "gateway streaming ingress failed to open events stream",
            );
            return;
        }
    };

    let consumer_config = match &spawn.kind {
        StreamingIngressKind::MessageStream { req_id } => {
            gateway_stream_events_consumer(&prefix, req_id, config.max_ack_pending.as_i64())
        }
        StreamingIngressKind::TasksResubscribe { task_id, last_seq, .. } => {
            resubscribe_consumer_with_flow(&prefix, task_id, *last_seq, config.max_ack_pending.as_i64())
        }
    };

    let consumer = match stream.create_consumer(consumer_config).await {
        Ok(consumer) => consumer,
        Err(error) => {
            warn!(error = %error, "gateway streaming ingress failed to create ephemeral consumer");
            return;
        }
    };

    // `consumer.fetch().max_messages(1).messages()` would yield ONE batch
    // and then close — the pump exited after a single event. Use the
    // continuous `messages()` stream so the pump keeps pulling until the
    // shutdown token fires or the consumer ends.
    let mut messages = match consumer.messages().await {
        Ok(messages) => messages,
        Err(error) => {
            warn!(error = %error, "gateway streaming ingress messages stream unavailable");
            return;
        }
    };

    debug!(
        reply = %spawn.reply,
        req_id = %spawn.req_id(),
        method = spawn.method_label(),
        "gateway streaming ingress pump started",
    );

    while !shutdown.is_cancelled() {
        let item = tokio::select! {
            _ = shutdown.cancelled() => break,
            item = messages.next() => item,
        };

        let Some(item) = item else {
            break;
        };

        let message = match item {
            Ok(message) => message,
            Err(error) => {
                warn!(error = %error, "gateway streaming ingress fetch item error");
                break;
            }
        };

        // JetStream tracks per-message delivery attempts; carry that count
        // so a permanently failing publish (bad reply subject, ACL, etc.)
        // doesn't NAK-loop forever — match the egress path's 3-attempt
        // budget before Term'ing.
        let attempt = message.info().map(|i| i.delivered).unwrap_or(1);
        if let Err(error) = client.publish(spawn.reply.clone(), message.payload.clone()).await {
            let disposition = if attempt >= STREAMING_INGRESS_MAX_FORWARD_ATTEMPTS {
                AckKind::Term
            } else {
                AckKind::Nak(None)
            };
            warn!(
                reply = %spawn.reply,
                error = %error,
                attempt,
                ?disposition,
                "gateway streaming ingress forward to caller reply failed",
            );
            let _ = message.ack_with(disposition).await;
            continue;
        }

        // `double_ack` waits for the JetStream server to confirm the ack so
        // a dropped ack doesn't trigger redelivery + duplicate publish to
        // the caller. On persistent ack failure we Term the message: the
        // payload already shipped to the caller, and another redelivery
        // would publish a duplicate.
        if let Err(error) = message.double_ack().await {
            warn!(
                error = %error,
                "gateway streaming ingress double-ack failed; terminating to avoid duplicate publish",
            );
            let _ = message.ack_with(AckKind::Term).await;
        }
    }

    debug!(reply = %spawn.reply, "gateway streaming ingress pump stopped");
}

/// Wire shape of the `tasks/resubscribe` request params. Parsing through a
/// dedicated struct surfaces "missing" vs. "malformed" vs. "invalid value"
/// as distinct typed errors instead of collapsing every failure into
/// `Option::None`.
#[derive(Debug, serde::Deserialize)]
pub struct ResubscribeParamsWire {
    #[serde(alias = "task_id", alias = "taskId")]
    pub id: Option<String>,
    #[serde(default, alias = "lastSeq")]
    pub last_seq: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResubscribeParamsError {
    #[error("tasks/resubscribe params: failed to deserialize")]
    Deserialize(#[source] serde_json::Error),
    #[error("tasks/resubscribe params: missing task id (expected `id`, `task_id`, or `taskId`)")]
    MissingTaskId,
    #[error("tasks/resubscribe params: task id failed validation")]
    InvalidTaskId(#[source] a2a_nats::TaskIdError),
}

/// Parse `tasks/resubscribe` params into a typed shape. Returns the
/// resolved task id plus the optional `last_seq` resume cursor (`0` when
/// absent — the consumer starts from the beginning).
pub fn parse_resubscribe_params(params: &serde_json::Value) -> Result<(A2aTaskId, u64), ResubscribeParamsError> {
    let wire: ResubscribeParamsWire =
        serde_json::from_value(params.clone()).map_err(ResubscribeParamsError::Deserialize)?;
    let raw = wire.id.ok_or(ResubscribeParamsError::MissingTaskId)?;
    let task_id = A2aTaskId::new(raw).map_err(ResubscribeParamsError::InvalidTaskId)?;
    Ok((task_id, wire.last_seq.unwrap_or(0)))
}

pub fn req_id_from_headers_or_payload(headers: &async_nats::HeaderMap, payload: &[u8]) -> Option<ReqId> {
    if let Some(value) = headers.get(a2a_nats::constants::REQ_ID_HEADER) {
        return Some(ReqId::from_header(value.as_str()));
    }
    a2a_nats::jsonrpc::extract_request_id_from_body(payload).map(|id| ReqId::from_header(id.to_string()))
}

#[cfg(test)]
mod tests;
