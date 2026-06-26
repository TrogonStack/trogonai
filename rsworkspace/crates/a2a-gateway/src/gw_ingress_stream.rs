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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayStreamingIngressConfig {
    pub max_ack_pending: i64,
    pub max_inflight_per_caller: usize,
}

impl GatewayStreamingIngressConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let max_ack_pending = env
            .var(ENV_GATEWAY_STREAMING_MAX_ACK_PENDING)
            .ok()
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .unwrap_or(DEFAULT_STREAMING_MAX_ACK_PENDING)
            .max(1);
        let max_inflight_per_caller = env
            .var(ENV_GATEWAY_STREAMING_MAX_INFLIGHT)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_STREAMING_MAX_INFLIGHT)
            .max(1);
        Self {
            max_ack_pending,
            max_inflight_per_caller,
        }
    }
}

pub fn gateway_streaming_ingress_enabled<E: ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var(ENV_GATEWAY_STREAMING_INGRESS) else {
        return false;
    };
    matches!(flag.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingIngressMethod {
    MessageStream,
    TasksResubscribe,
}

pub struct StreamingIngressSpawn {
    pub method: StreamingIngressMethod,
    pub req_id: ReqId,
    pub task_id: Option<A2aTaskId>,
    pub last_seq: Option<u64>,
    pub reply: async_nats::Subject,
    pub caller_key: String,
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
            inner: std::sync::Arc::new(CallerInflightGate::new(config.max_inflight_per_caller)),
        }
    }

    #[must_use]
    fn try_acquire<'a>(&'a self, caller_key: &str) -> Option<CallerInflightPermit<'a>> {
        self.inner.try_acquire(caller_key)
    }
}

// Under `cfg(coverage)` the pump function is stubbed and never touches the
// gate — silence dead-code warnings without losing the unit-test coverage
// the gate gets via direct `try_acquire` calls.
#[cfg_attr(coverage, allow(dead_code))]
struct CallerInflightGate {
    limit: usize,
    inflight: Mutex<HashMap<String, usize>>,
}

#[cfg_attr(coverage, allow(dead_code))]
impl CallerInflightGate {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    fn try_acquire(&self, caller_key: &str) -> Option<CallerInflightPermit<'_>> {
        // Recover from a poisoned lock — the inflight map is plain counters,
        // a partial inc/dec can't leave it logically corrupt, and panicking
        // every caller after an unrelated panic would turn the gate into a
        // service-wide DoS.
        let mut guard = match self.inflight.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let count = guard.entry(caller_key.to_string()).or_insert(0);
        if *count >= self.limit {
            return None;
        }
        *count += 1;
        Some(CallerInflightPermit {
            gate: self,
            caller_key: caller_key.to_string(),
        })
    }
}

#[cfg_attr(coverage, allow(dead_code))]
struct CallerInflightPermit<'a> {
    gate: &'a CallerInflightGate,
    caller_key: String,
}

impl Drop for CallerInflightPermit<'_> {
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

/// Spawn the streaming-ingress pump on a background task. The pump owns its
/// JetStream consumer and forwards events to the caller's `reply` until
/// shutdown fires or the consumer ends.
///
/// `gate` MUST be the shared `StreamingIngressGate` the runtime built once
/// at boot — passing a fresh gate per spawn would disable
/// `max_inflight_per_caller`. Gated behind `cfg(not(coverage))` because the
/// pump binds a real JetStream context.
#[cfg(not(coverage))]
pub fn spawn_streaming_ingress_pump(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayStreamingIngressConfig,
    gate: StreamingIngressGate,
    spawn: StreamingIngressSpawn,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        run_streaming_ingress_pump(client, prefix, config, gate, spawn, shutdown).await;
    });
}

#[cfg(coverage)]
pub fn spawn_streaming_ingress_pump(
    _client: async_nats::Client,
    _prefix: A2aPrefix,
    _config: GatewayStreamingIngressConfig,
    _gate: StreamingIngressGate,
    _spawn: StreamingIngressSpawn,
    _shutdown: CancellationToken,
) {
}

#[cfg(not(coverage))]
async fn run_streaming_ingress_pump(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayStreamingIngressConfig,
    gate: StreamingIngressGate,
    spawn: StreamingIngressSpawn,
    shutdown: CancellationToken,
) {
    // Hold a permit from the SHARED gate for the pump's whole lifetime so
    // `max_inflight_per_caller` actually constrains concurrent pumps for
    // the same caller. A per-pump gate would let a single caller open
    // unlimited concurrent streams.
    let Some(_permit) = gate.try_acquire(&spawn.caller_key) else {
        warn!(
            caller = %spawn.caller_key,
            method = ?spawn.method,
            "gateway streaming ingress dropped — per-caller inflight limit",
        );
        return;
    };

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

    let consumer_config = match spawn.method {
        StreamingIngressMethod::MessageStream => {
            gateway_stream_events_consumer(&prefix, &spawn.req_id, config.max_ack_pending)
        }
        StreamingIngressMethod::TasksResubscribe => {
            // The dispatcher only spawns `TasksResubscribe` with a resolved
            // task_id; the verify happens at the parse seam in
            // task_id_from_resubscribe_params.
            let Some(task_id) = spawn.task_id.clone() else {
                warn!(
                    method = ?spawn.method,
                    "gateway streaming ingress dropped — tasks/resubscribe missing task_id",
                );
                return;
            };
            let last_seq = spawn.last_seq.unwrap_or(0);
            resubscribe_consumer_with_flow(&prefix, &task_id, last_seq, config.max_ack_pending)
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
    // shutdown token fires or the consumer ends. The expiry/heartbeat hints
    // are passed at consumer creation rather than here.
    let mut messages = match consumer.messages().await {
        Ok(messages) => messages,
        Err(error) => {
            warn!(error = %error, "gateway streaming ingress messages stream unavailable");
            return;
        }
    };

    debug!(
        reply = %spawn.reply,
        req_id = %spawn.req_id,
        method = ?spawn.method,
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

        if let Err(error) = client.publish(spawn.reply.clone(), message.payload.clone()).await {
            warn!(
                reply = %spawn.reply,
                error = %error,
                "gateway streaming ingress forward to caller reply failed",
            );
            let _ = message.ack_with(AckKind::Nak(None)).await;
            continue;
        }

        if let Err(error) = message.ack().await {
            warn!(error = %error, "gateway streaming ingress ack failed");
        }
    }

    debug!(reply = %spawn.reply, "gateway streaming ingress pump stopped");
}

pub fn task_id_from_resubscribe_params(params: &serde_json::Value) -> Option<A2aTaskId> {
    params
        .get("id")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("taskId"))
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| A2aTaskId::new(raw).ok())
}

pub fn last_seq_from_resubscribe_params(params: &serde_json::Value) -> Option<u64> {
    params
        .get("last_seq")
        .or_else(|| params.get("lastSeq"))
        .and_then(serde_json::Value::as_u64)
}

pub fn req_id_from_headers_or_payload(headers: &async_nats::HeaderMap, payload: &[u8]) -> Option<ReqId> {
    if let Some(value) = headers.get(a2a_nats::constants::REQ_ID_HEADER) {
        return Some(ReqId::from_header(value.as_str()));
    }
    a2a_nats::jsonrpc::extract_request_id_from_body(payload).map(|id| ReqId::from_header(id.to_string()))
}

#[cfg(test)]
mod tests;
