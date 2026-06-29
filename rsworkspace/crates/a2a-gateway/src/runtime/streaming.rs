//! Streaming-ingress dispatch wrapper.
//!
//! The dispatch path encodes "is this a streaming method?" exactly
//! once, here, so the orchestrator can stay a single call sequence
//! that fans out to the streaming pump when (and only when) the
//! method dots resolve to `message.stream` or `tasks.resubscribe`.
//!
//! `tasks.resubscribe` carries the resume coordinates (`task_id`,
//! `last_seq`) inside its JSON-RPC params; the wrapper parses them
//! once and hands typed values to the pump so the pump never has to
//! reason about wire shapes.

use a2a_nats::A2aTaskId;
use serde_json::Value;

use crate::gw_ingress_stream::{
    CallerKey, GatewayStreamingIngressConfig, StreamingIngressGate, StreamingIngressKind, StreamingIngressSpawn,
    StreamingIngressSpawnError, req_id_from_headers_or_payload, spawn_streaming_ingress_pump,
};
use crate::runtime::env::json_rpc_params;

/// Method-dots string for the unary version of `message/send`.
const MESSAGE_STREAM_METHOD_DOTS: &str = "message.stream";
const TASKS_RESUBSCRIBE_METHOD_DOTS: &str = "tasks.resubscribe";

/// Outcome of [`maybe_spawn_streaming_ingress_pump`].
///
/// `NotStreaming` is intentionally distinct from `Spawned(_)` so the
/// dispatch path can branch on "we handled this" vs "fall through to
/// the unary path" without inspecting a sentinel value.
#[derive(Debug)]
pub enum MaybeStreamingSpawn {
    /// The method isn't streaming -- dispatch should continue down
    /// the unary path.
    NotStreaming,
    /// A streaming pump was attempted; the inner `Result` carries
    /// whether the gate refused (per-caller limit) or the pump was
    /// successfully detached.
    Spawned(Result<(), StreamingIngressSpawnError>),
}

/// Pure decision of "what (if anything) to spawn" for this method
/// dots + payload pair. Split out from
/// [`maybe_spawn_streaming_ingress_pump`] so the wire-parsing branches
/// stay unit-testable without standing up a real NATS client.
#[derive(Debug)]
pub enum StreamingSpawnIntent {
    NotStreaming,
    Spawn(StreamingIngressKind),
}

#[must_use]
pub fn classify_streaming_spawn(
    method_dots: &str,
    headers: &async_nats::HeaderMap,
    payload: &[u8],
) -> StreamingSpawnIntent {
    let Some(req_id) = req_id_from_headers_or_payload(headers, payload) else {
        return StreamingSpawnIntent::NotStreaming;
    };
    match method_dots {
        MESSAGE_STREAM_METHOD_DOTS => StreamingSpawnIntent::Spawn(StreamingIngressKind::MessageStream { req_id }),
        TASKS_RESUBSCRIBE_METHOD_DOTS => {
            let params = json_rpc_params(payload);
            let Some(task_id) = task_id_from_resubscribe_params(&params) else {
                return StreamingSpawnIntent::NotStreaming;
            };
            let last_seq = last_seq_from_resubscribe_params(&params).unwrap_or(0);
            StreamingSpawnIntent::Spawn(StreamingIngressKind::TasksResubscribe {
                req_id,
                task_id,
                last_seq,
            })
        }
        _ => StreamingSpawnIntent::NotStreaming,
    }
}

/// Extract the resume task id from a `tasks/resubscribe` params
/// object. Accepts the three canonical field names (`id`, `task_id`,
/// `taskId`) so clients written against either the JSON-RPC contract
/// or the more verbose dispatch schema land at the same typed id.
#[must_use]
pub fn task_id_from_resubscribe_params(params: &Value) -> Option<A2aTaskId> {
    params
        .get("id")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("taskId"))
        .and_then(Value::as_str)
        .and_then(|raw| A2aTaskId::new(raw).ok())
}

/// Extract the resume sequence number from a `tasks/resubscribe`
/// params object. Absence means "start from the latest" -- the pump
/// treats `None` as "no resume cursor" rather than "resume at 0".
#[must_use]
pub fn last_seq_from_resubscribe_params(params: &Value) -> Option<u64> {
    params
        .get("last_seq")
        .or_else(|| params.get("lastSeq"))
        .and_then(Value::as_u64)
}

/// Spawn the streaming-ingress pump if the method dots designate a
/// streaming method (`message.stream`, `tasks.resubscribe`).
/// Returns [`MaybeStreamingSpawn::NotStreaming`] otherwise so the
/// dispatch path can fall through to the unary handler.
///
/// `tasks.resubscribe` requires a task id in its params; an absent
/// or invalid id returns `NotStreaming` so the request lands on the
/// unary error path rather than spawning a pump with an unknown
/// stream cursor.
#[allow(clippy::too_many_arguments)]
pub fn maybe_spawn_streaming_ingress_pump(
    client: &async_nats::Client,
    prefix: &a2a_nats::A2aPrefix,
    streaming_ingress_config: GatewayStreamingIngressConfig,
    gate: &StreamingIngressGate,
    shutdown: tokio_util::sync::CancellationToken,
    method_dots: &str,
    headers: &async_nats::HeaderMap,
    payload: &[u8],
    reply: async_nats::Subject,
    caller_key: CallerKey,
) -> MaybeStreamingSpawn {
    let StreamingSpawnIntent::Spawn(kind) = classify_streaming_spawn(method_dots, headers, payload) else {
        return MaybeStreamingSpawn::NotStreaming;
    };
    let spawn = StreamingIngressSpawn {
        kind,
        reply,
        caller_key,
    };
    MaybeStreamingSpawn::Spawned(spawn_streaming_ingress_pump(
        client.clone(),
        prefix.clone(),
        streaming_ingress_config,
        gate.clone(),
        spawn,
        shutdown,
    ))
}

#[cfg(test)]
mod tests;
