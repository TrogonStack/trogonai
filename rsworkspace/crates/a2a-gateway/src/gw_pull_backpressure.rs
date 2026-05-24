//! Gateway pull consumer for task-event egress (`A2A_EVENTS`) with JetStream flow control.
//!
//! Operator guide: [`../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_nats::jetstream::consumers::gateway_events_consumer;
use a2a_nats::jetstream::streams::events_stream_name;
use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use async_nats::jetstream::consumer::AckPolicy;
use async_nats::jetstream::{self, AckKind};
use bytes::Bytes;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use trogon_std::env::ReadEnv;

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

const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const FETCH_EXPIRES: Duration = Duration::from_secs(30);

/// JetStream durable name for the gateway task-event egress consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventsConsumerDurable(String);

impl EventsConsumerDurable {
    pub fn for_prefix(prefix: &A2aPrefix) -> Self {
        Self(format!(
            "{}_GATEWAY_EVENTS",
            prefix.as_str().to_uppercase().replace('.', "_")
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded in-flight unacked messages for the gateway pull consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayEventsMaxAckPending(usize);

impl GatewayEventsMaxAckPending {
    pub fn new(value: usize) -> Self {
        Self(value.max(1))
    }

    pub fn as_i64(self) -> i64 {
        i64::try_from(self.0).unwrap_or(i64::MAX)
    }

    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// Pull fetch batch size (JetStream flow-control boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayEventsFetchBatch(usize);

impl GatewayEventsFetchBatch {
    pub fn new(value: usize) -> Self {
        Self(value.max(1))
    }

    pub fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullConsumerHints {
    pub max_ack_pending: usize,
    pub inactive_threshold_secs: u64,
    pub ack_policy: AckPolicy,
}

impl PullConsumerHints {
    pub const fn gateway_baseline() -> Self {
        Self {
            max_ack_pending: DEFAULT_MAX_ACK_PENDING,
            inactive_threshold_secs: DEFAULT_INACTIVE_THRESHOLD_SECS,
            ack_policy: AckPolicy::Explicit,
        }
    }

    pub fn inactive_threshold(&self) -> Duration {
        Duration::from_secs(self.inactive_threshold_secs)
    }
}

impl Default for PullConsumerHints {
    fn default() -> Self {
        Self::gateway_baseline()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressAckDisposition {
    Ack,
    Nak { delay: Option<Duration> },
    Term,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResubscribeEgressPlan {
    pub filter_subject: String,
    pub start_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageStreamEgressPlan {
    pub filter_subject: String,
    pub hints: PullConsumerHints,
}

pub trait TaskEventsEgressPlanner {
    type Error: std::error::Error + Send + Sync + 'static;

    fn pull_hints(&self) -> PullConsumerHints;

    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        req_id: &ReqId,
    ) -> Result<MessageStreamEgressPlan, Self::Error>;

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribeEgressPlan, Self::Error>;

    fn forward_disposition(
        &self,
        attempt: u32,
        forward_error: Option<&str>,
    ) -> EgressAckDisposition;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BaselineTaskEventsEgressPlanner {
    hints: PullConsumerHints,
}

impl BaselineTaskEventsEgressPlanner {
    pub const fn new() -> Self {
        Self {
            hints: PullConsumerHints::gateway_baseline(),
        }
    }
}

impl TaskEventsEgressPlanner for BaselineTaskEventsEgressPlanner {
    type Error = std::convert::Infallible;

    fn pull_hints(&self) -> PullConsumerHints {
        self.hints
    }

    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        req_id: &ReqId,
    ) -> Result<MessageStreamEgressPlan, Self::Error> {
        Ok(MessageStreamEgressPlan {
            filter_subject: format!("{}.task.*.events.{req_id}", prefix.as_str()),
            hints: self.hints,
        })
    }

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribeEgressPlan, Self::Error> {
        Ok(ResubscribeEgressPlan {
            filter_subject: format!("{}.task.{task_id}.events.*", prefix.as_str()),
            start_sequence: last_seq.saturating_add(1),
        })
    }

    fn forward_disposition(
        &self,
        attempt: u32,
        forward_error: Option<&str>,
    ) -> EgressAckDisposition {
        match forward_error {
            None => EgressAckDisposition::Ack,
            Some(_) if attempt < 3 => EgressAckDisposition::Nak { delay: None },
            Some(_) => EgressAckDisposition::Term,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayEventsPullConfig {
    pub max_ack_pending: GatewayEventsMaxAckPending,
    pub fetch_batch: GatewayEventsFetchBatch,
    pub fetch_heartbeat: Duration,
    pub max_inflight_per_caller: usize,
}

impl GatewayEventsPullConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let max_ack_pending = env
            .var(ENV_GATEWAY_EVENTS_MAX_ACK_PENDING)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_ACK_PENDING);
        let fetch_batch = env
            .var(ENV_GATEWAY_EVENTS_FETCH_BATCH)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_FETCH_BATCH);
        let fetch_heartbeat_secs = env
            .var(ENV_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_FETCH_HEARTBEAT_SECS)
            .max(1);
        let max_inflight_per_caller = env
            .var(ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_INFLIGHT_PER_CALLER)
            .max(1);

        Self {
            max_ack_pending: GatewayEventsMaxAckPending::new(max_ack_pending),
            fetch_batch: GatewayEventsFetchBatch::new(fetch_batch),
            fetch_heartbeat: Duration::from_secs(fetch_heartbeat_secs),
            max_inflight_per_caller,
        }
    }
}

pub fn gateway_events_pull_enabled<E: ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var(ENV_GATEWAY_EVENTS_PULL) else {
        return false;
    };
    matches!(
        flag.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn gateway_egress_subject(prefix: &A2aPrefix, req_id: &ReqId) -> String {
    format!("{}.gateway.egress.{}", prefix.as_str(), req_id.as_str())
}

pub fn parse_task_events_subject(prefix: &str, subject: &str) -> Option<(A2aTaskId, ReqId)> {
    let expected = format!("{prefix}.task.");
    let rest = subject.strip_prefix(&expected)?;
    let (task_id, req_id) = rest.split_once(".events.")?;
    Some((A2aTaskId::new(task_id).ok()?, ReqId::from_header(req_id)))
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

pub async fn run_gateway_events_pull(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayEventsPullConfig,
    shutdown: CancellationToken,
) {
    let planner = BaselineTaskEventsEgressPlanner::new();
    let hints = planner.pull_hints();
    let durable = EventsConsumerDurable::for_prefix(&prefix);
    let inflight_gate = Arc::new(CallerInflightGate::new(config.max_inflight_per_caller));
    let mut backoff = INITIAL_BACKOFF;

    info_span_start(&prefix, &durable, &config);

    while !shutdown.is_cancelled() {
        let cycle = run_fetch_cycle(
            &client,
            &prefix,
            &durable,
            &planner,
            &config,
            inflight_gate.as_ref(),
            hints,
        )
        .await;

        match cycle {
            Ok(()) => {
                backoff = INITIAL_BACKOFF;
            }
            Err(error) => {
                warn!(
                    prefix = %prefix,
                    durable = %durable.as_str(),
                    error = %error,
                    backoff_ms = backoff.as_millis(),
                    "gateway events pull cycle failed; backing off"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    debug!(prefix = %prefix, "gateway events pull loop stopped");
}

fn info_span_start(prefix: &A2aPrefix, durable: &EventsConsumerDurable, config: &GatewayEventsPullConfig) {
    tracing::info!(
        prefix = %prefix,
        durable = %durable.as_str(),
        max_ack_pending = config.max_ack_pending.as_usize(),
        fetch_batch = config.fetch_batch.as_usize(),
        fetch_heartbeat_secs = config.fetch_heartbeat.as_secs(),
        max_inflight_per_caller = config.max_inflight_per_caller,
        "gateway events pull consumer started"
    );
}

async fn run_fetch_cycle(
    client: &async_nats::Client,
    prefix: &A2aPrefix,
    durable: &EventsConsumerDurable,
    planner: &BaselineTaskEventsEgressPlanner,
    config: &GatewayEventsPullConfig,
    inflight_gate: &CallerInflightGate,
    hints: PullConsumerHints,
) -> Result<(), String> {
    let jetstream = jetstream::new(client.clone());
    let stream_name = events_stream_name(prefix);
    let stream = jetstream
        .get_stream(&stream_name)
        .await
        .map_err(|e| format!("get_stream {stream_name}: {e}"))?;

    let consumer_config = gateway_events_consumer(
        prefix,
        durable.as_str(),
        config.max_ack_pending.as_i64(),
    );
    let consumer = stream
        .get_or_create_consumer(durable.as_str(), consumer_config)
        .await
        .map_err(|e| format!("get_or_create_consumer {}: {e}", durable.as_str()))?;

    let mut batch = consumer
        .fetch()
        .max_messages(config.fetch_batch.as_usize())
        .expires(FETCH_EXPIRES)
        .heartbeat(config.fetch_heartbeat)
        .messages()
        .await
        .map_err(|e| format!("fetch.messages: {e}"))?;

    while let Some(item) = batch.next().await {
        let message = item.map_err(|e| format!("fetch item: {e}"))?;
        let subject = message.subject.as_str();
        let Some((_task_id, req_id)) = parse_task_events_subject(prefix.as_str(), subject) else {
            message
                .ack_with(AckKind::Term)
                .await
                .map_err(|e| format!("term unparseable subject {subject}: {e}"))?;
            continue;
        };

        let caller_key = req_id.as_str().to_string();
        let Some(_permit) = inflight_gate.try_acquire(&caller_key) else {
            message
                .ack_with(AckKind::Nak(None))
                .await
                .map_err(|e| format!("nak inflight limit {subject}: {e}"))?;
            continue;
        };

        let attempt = 1u32;
        let forward_result = forward_task_event(client, prefix, &req_id, &message.payload).await;
        match planner.forward_disposition(attempt, forward_result.err().as_deref()) {
            EgressAckDisposition::Ack => {
                message
                    .ack()
                    .await
                    .map_err(|e| format!("ack {subject}: {e}"))?;
            }
            EgressAckDisposition::Nak { delay } => {
                message
                    .ack_with(AckKind::Nak(delay))
                    .await
                    .map_err(|e| format!("nak {subject}: {e}"))?;
            }
            EgressAckDisposition::Term => {
                message
                    .ack_with(AckKind::Term)
                    .await
                    .map_err(|e| format!("term {subject}: {e}"))?;
            }
        }
    }

    let _ = hints;
    Ok(())
}

async fn forward_task_event(
    client: &async_nats::Client,
    prefix: &A2aPrefix,
    req_id: &ReqId,
    payload: &Bytes,
) -> Result<(), String> {
    let subject = gateway_egress_subject(prefix, req_id);
    client
        .publish(subject, payload.clone())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use trogon_std::env::InMemoryEnv;

    use super::*;

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).expect("test prefix")
    }

    #[test]
    fn durable_name_scoped_to_prefix() {
        assert_eq!(
            EventsConsumerDurable::for_prefix(&prefix()).as_str(),
            "A2A_GATEWAY_EVENTS"
        );
    }

    #[test]
    fn parse_task_events_subject_extracts_ids() {
        let (task_id, req_id) =
            parse_task_events_subject("a2a", "a2a.task.t1.events.r1").expect("parsed");
        assert_eq!(task_id.as_str(), "t1");
        assert_eq!(req_id.as_str(), "r1");
    }

    #[test]
    fn planner_forward_disposition_retries_then_terms() {
        let planner = BaselineTaskEventsEgressPlanner::new();
        assert_eq!(
            planner.forward_disposition(1, Some("fail")),
            EgressAckDisposition::Nak { delay: None }
        );
        assert_eq!(
            planner.forward_disposition(3, Some("fail")),
            EgressAckDisposition::Term
        );
        assert_eq!(planner.forward_disposition(1, None), EgressAckDisposition::Ack);
    }

    #[test]
    fn gateway_events_pull_enabled_reads_on_flag() {
        let env = InMemoryEnv::new();
        assert!(!gateway_events_pull_enabled(&env));
        env.set(ENV_GATEWAY_EVENTS_PULL, "on");
        assert!(gateway_events_pull_enabled(&env));
    }

    #[test]
    fn config_from_env_applies_overrides() {
        let env = InMemoryEnv::new();
        env.set(ENV_GATEWAY_EVENTS_MAX_ACK_PENDING, "512");
        env.set(ENV_GATEWAY_EVENTS_FETCH_BATCH, "4");
        env.set(ENV_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS, "10");
        env.set(ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER, "8");
        let cfg = GatewayEventsPullConfig::from_env(&env);
        assert_eq!(cfg.max_ack_pending.as_usize(), 512);
        assert_eq!(cfg.fetch_batch.as_usize(), 4);
        assert_eq!(cfg.fetch_heartbeat, Duration::from_secs(10));
        assert_eq!(cfg.max_inflight_per_caller, 8);
    }

    #[test]
    fn gateway_egress_subject_includes_req_id() {
        let req_id = ReqId::from_header("req-1");
        assert_eq!(
            gateway_egress_subject(&prefix(), &req_id),
            "a2a.gateway.egress.req-1"
        );
    }

    #[test]
    fn caller_inflight_gate_enforces_limit() {
        let gate = CallerInflightGate::new(1);
        let first = gate.try_acquire("caller-a");
        assert!(first.is_some());
        assert!(gate.try_acquire("caller-a").is_none());
        drop(first);
        assert!(gate.try_acquire("caller-a").is_some());
    }
}
