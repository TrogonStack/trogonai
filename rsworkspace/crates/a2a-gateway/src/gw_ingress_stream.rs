//! Gateway-owned JetStream pull pipe for streaming ingress (`message/stream`, `tasks/resubscribe`).
//!
//! Operator guide: [`../../../../docs/a2a/how-to/operators/streaming-backpressure.md`](../../../../docs/a2a/how-to/operators/streaming-backpressure.md).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_nats::jetstream::consumers::{gateway_stream_events_consumer, resubscribe_consumer_with_flow};
use a2a_nats::jetstream::streams::events_stream_name;
use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use async_nats::jetstream::{self, AckKind};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use trogon_std::env::ReadEnv;

pub const ENV_GATEWAY_STREAMING_INGRESS: &str = "A2A_GATEWAY_STREAMING_INGRESS";
pub const ENV_GATEWAY_STREAMING_MAX_ACK_PENDING: &str = "A2A_GATEWAY_STREAMING_MAX_ACK_PENDING";
pub const ENV_GATEWAY_STREAMING_MAX_INFLIGHT: &str = "A2A_GATEWAY_STREAMING_MAX_INFLIGHT";

pub const DEFAULT_STREAMING_MAX_ACK_PENDING: i64 = 32;
pub const DEFAULT_STREAMING_MAX_INFLIGHT: usize = 32;
const FETCH_EXPIRES: Duration = Duration::from_secs(30);
const FETCH_HEARTBEAT: Duration = Duration::from_secs(5);

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
    matches!(
        flag.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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

struct CallerInflightGate {
    limit: usize,
    inflight: Mutex<std::collections::HashMap<String, usize>>,
}

impl CallerInflightGate {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            inflight: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn try_acquire(&self, caller_key: &str) -> Option<CallerInflightPermit<'_>> {
        let mut guard = self.inflight.lock().unwrap();
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

struct CallerInflightPermit<'a> {
    gate: &'a CallerInflightGate,
    caller_key: String,
}

impl Drop for CallerInflightPermit<'_> {
    fn drop(&mut self) {
        let mut guard = self.gate.inflight.lock().unwrap();
        if let Some(count) = guard.get_mut(&self.caller_key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                guard.remove(&self.caller_key);
            }
        }
    }
}

pub fn spawn_streaming_ingress_pump(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayStreamingIngressConfig,
    spawn: StreamingIngressSpawn,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        run_streaming_ingress_pump(client, prefix, config, spawn, shutdown).await;
    });
}

async fn run_streaming_ingress_pump(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayStreamingIngressConfig,
    spawn: StreamingIngressSpawn,
    shutdown: CancellationToken,
) {
    let inflight_gate = Arc::new(CallerInflightGate::new(config.max_inflight_per_caller));
    let Some(_permit) = inflight_gate.try_acquire(&spawn.caller_key) else {
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
            let task_id = spawn.task_id.clone().expect("resubscribe requires task_id");
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

    let mut messages = match consumer
        .fetch()
        .max_messages(1)
        .expires(FETCH_EXPIRES)
        .heartbeat(FETCH_HEARTBEAT)
        .messages()
        .await
    {
        Ok(messages) => messages,
        Err(error) => {
            warn!(error = %error, "gateway streaming ingress fetch setup failed");
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

        if let Err(error) = client
            .publish(spawn.reply.clone(), message.payload.clone())
            .await
        {
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

pub fn req_id_from_headers_or_payload(
    headers: &async_nats::HeaderMap,
    payload: &[u8],
) -> Option<ReqId> {
    if let Some(value) = headers.get(a2a_nats::constants::REQ_ID_HEADER) {
        return Some(ReqId::from_header(value.as_str()));
    }
    a2a_nats::extract_request_id(payload).map(|id| ReqId::from_header(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resubscribe_params_extract_task_and_seq() {
        let params = serde_json::json!({"id": "task-1", "last_seq": 41});
        let task_id = task_id_from_resubscribe_params(&params).expect("task");
        assert_eq!(task_id.as_str(), "task-1");
        assert_eq!(last_seq_from_resubscribe_params(&params), Some(41));
    }

    #[test]
    fn streaming_ingress_enabled_reads_flag() {
        let env = trogon_std::env::InMemoryEnv::new();
        assert!(!gateway_streaming_ingress_enabled(&env));
        env.set(ENV_GATEWAY_STREAMING_INGRESS, "on");
        assert!(gateway_streaming_ingress_enabled(&env));
    }
}
