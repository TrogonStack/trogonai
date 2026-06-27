//! Gateway pull consumer for task-event egress (`A2A_EVENTS`) with
//! JetStream flow control.
//!
//! Operator guide:
//! [`../../../../docs/a2a/how-to/operators/streaming-backpressure.md`](../../../../docs/a2a/how-to/operators/streaming-backpressure.md).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use async_nats::jetstream::consumer::AckPolicy;
use tokio_util::sync::CancellationToken;
use trogon_std::env::ReadEnv;

// Pump-only imports — gated so `cfg(coverage)` doesn't warn on unused
// imports when the pull-consumer loop stubs out.
#[cfg(not(coverage))]
use a2a_nats::jetstream::consumers::gateway_events_consumer;
#[cfg(not(coverage))]
use a2a_nats::jetstream::streams::events_stream_name;
#[cfg(not(coverage))]
use async_nats::jetstream::{self, AckKind};
#[cfg(not(coverage))]
use bytes::Bytes;
#[cfg(not(coverage))]
use futures::StreamExt;
#[cfg(not(coverage))]
use tracing::{debug, warn};

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

#[cfg(not(coverage))]
const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
#[cfg(not(coverage))]
const MAX_BACKOFF: Duration = Duration::from_secs(30);
#[cfg(not(coverage))]
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

/// Per-caller concurrent in-flight cap for the gateway events pump. Floored
/// at 1 so the gate always lets at least one message through per caller —
/// a config carrying `0` would NAK every message into a tight loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayEventsMaxInflightPerCaller(usize);

impl GatewayEventsMaxInflightPerCaller {
    #[must_use]
    pub fn new(value: usize) -> Self {
        Self(value.max(1))
    }

    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// Pull-consumer fetch heartbeat. Floored at one second so a config carrying
/// `Duration::ZERO` can't disable JetStream's heartbeat-driven liveness
/// detection, which would silently turn fetch errors into hangs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayEventsFetchHeartbeat(Duration);

impl GatewayEventsFetchHeartbeat {
    #[must_use]
    pub fn new(value: Duration) -> Self {
        Self(value.max(Duration::from_secs(1)))
    }

    pub fn as_duration(self) -> Duration {
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

    fn plan_message_stream(&self, prefix: &A2aPrefix, req_id: &ReqId) -> Result<MessageStreamEgressPlan, Self::Error>;

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribeEgressPlan, Self::Error>;

    fn forward_disposition(&self, attempt: u32, forward_error: Option<&str>) -> EgressAckDisposition;
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

    fn plan_message_stream(&self, prefix: &A2aPrefix, req_id: &ReqId) -> Result<MessageStreamEgressPlan, Self::Error> {
        Ok(MessageStreamEgressPlan {
            filter_subject: format!("{}.tasks.*.events.{req_id}", prefix.as_str()),
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
            filter_subject: format!("{}.tasks.{task_id}.events.*", prefix.as_str()),
            start_sequence: last_seq.saturating_add(1),
        })
    }

    fn forward_disposition(&self, attempt: u32, forward_error: Option<&str>) -> EgressAckDisposition {
        match forward_error {
            None => EgressAckDisposition::Ack,
            Some(_) if attempt < 3 => EgressAckDisposition::Nak { delay: None },
            Some(_) => EgressAckDisposition::Term,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayEventsPullConfig {
    max_ack_pending: GatewayEventsMaxAckPending,
    fetch_batch: GatewayEventsFetchBatch,
    fetch_heartbeat: GatewayEventsFetchHeartbeat,
    max_inflight_per_caller: GatewayEventsMaxInflightPerCaller,
}

impl GatewayEventsPullConfig {
    #[must_use]
    pub fn new(
        max_ack_pending: GatewayEventsMaxAckPending,
        fetch_batch: GatewayEventsFetchBatch,
        fetch_heartbeat: GatewayEventsFetchHeartbeat,
        max_inflight_per_caller: GatewayEventsMaxInflightPerCaller,
    ) -> Self {
        Self {
            max_ack_pending,
            fetch_batch,
            fetch_heartbeat,
            max_inflight_per_caller,
        }
    }

    pub fn max_ack_pending(&self) -> GatewayEventsMaxAckPending {
        self.max_ack_pending
    }

    pub fn fetch_batch(&self) -> GatewayEventsFetchBatch {
        self.fetch_batch
    }

    pub fn fetch_heartbeat(&self) -> GatewayEventsFetchHeartbeat {
        self.fetch_heartbeat
    }

    pub fn max_inflight_per_caller(&self) -> GatewayEventsMaxInflightPerCaller {
        self.max_inflight_per_caller
    }

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
            .unwrap_or(DEFAULT_FETCH_HEARTBEAT_SECS);
        let max_inflight_per_caller = env
            .var(ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_INFLIGHT_PER_CALLER);

        Self {
            max_ack_pending: GatewayEventsMaxAckPending::new(max_ack_pending),
            fetch_batch: GatewayEventsFetchBatch::new(fetch_batch),
            fetch_heartbeat: GatewayEventsFetchHeartbeat::new(Duration::from_secs(fetch_heartbeat_secs)),
            max_inflight_per_caller: GatewayEventsMaxInflightPerCaller::new(max_inflight_per_caller),
        }
    }
}

pub fn gateway_events_pull_enabled<E: ReadEnv>(env: &E) -> bool {
    let Ok(flag) = env.var(ENV_GATEWAY_EVENTS_PULL) else {
        return false;
    };
    matches!(flag.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

pub fn gateway_egress_subject(prefix: &A2aPrefix, req_id: &ReqId) -> String {
    format!("{}.gateway.egress.{}", prefix.as_str(), req_id.as_str())
}

pub fn parse_task_events_subject(prefix: &str, subject: &str) -> Option<(A2aTaskId, ReqId)> {
    // JetStream task event subjects are `{prefix}.tasks.{task_id}.events.{req_id}`.
    // Earlier versions of this helper stripped `.task.` (singular) and
    // dropped every pulled message via Term — coverage above hits both
    // happy and reject paths now.
    let expected = format!("{prefix}.tasks.");
    let rest = subject.strip_prefix(&expected)?;
    let (task_id, req_id) = rest.split_once(".events.")?;
    Some((A2aTaskId::new(task_id).ok()?, ReqId::from_header(req_id)))
}

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

    /// Take a permit. The permit owns an `Arc` back to the gate so it can
    /// move into a spawned task and release on drop — without that, the
    /// fetch loop could only hand out borrowed permits and the per-caller
    /// limit would only ever bind one message at a time per caller (i.e. a
    /// no-op once forward work is spawned off the loop).
    fn try_acquire(self: Arc<Self>, caller_key: &str) -> Option<CallerInflightPermit> {
        // Recover from a poisoned lock — see the matching note in
        // `gw_ingress_stream::CallerInflightGate`.
        let key = caller_key.to_string();
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

#[cfg_attr(coverage, allow(dead_code))]
struct CallerInflightPermit {
    gate: Arc<CallerInflightGate>,
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

/// Typed failure surface for the pull-cycle loop. Replaces the previous
/// `Result<(), String>` so structured source errors (jetstream binding,
/// consumer provisioning, fetch, ack) survive across the loop instead of
/// being flattened into a Display string.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(coverage, allow(dead_code))]
pub enum PullCycleError {
    #[error("bind events stream {stream}")]
    BindStream {
        stream: String,
        #[source]
        source: async_nats::jetstream::context::GetStreamError,
    },
    #[error("provision pull consumer {durable}")]
    ProvisionConsumer {
        durable: String,
        #[source]
        source: async_nats::jetstream::stream::ConsumerError,
    },
    #[error("fetch messages from {durable}")]
    FetchMessages {
        durable: String,
        #[source]
        source: async_nats::jetstream::consumer::pull::BatchError,
    },
    #[error("fetch yielded an error item on {subject}")]
    FetchItem {
        subject: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("ack {subject}")]
    Ack {
        subject: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Run the gateway egress pull-consumer loop. The body that binds a real
/// JetStream context is gated behind `cfg(not(coverage))`; the pure
/// planning + parsing helpers are exercised by unit tests under all builds.
#[cfg(not(coverage))]
pub async fn run_gateway_events_pull(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayEventsPullConfig,
    shutdown: CancellationToken,
) {
    let planner = BaselineTaskEventsEgressPlanner::new();
    let durable = EventsConsumerDurable::for_prefix(&prefix);
    let inflight_gate = Arc::new(CallerInflightGate::new(config.max_inflight_per_caller().as_usize()));
    let mut backoff = INITIAL_BACKOFF;

    info_span_start(&prefix, &durable, &config);

    while !shutdown.is_cancelled() {
        let cycle = run_fetch_cycle(
            &client,
            &prefix,
            &durable,
            &planner,
            &config,
            Arc::clone(&inflight_gate),
            shutdown.clone(),
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
                // Observe shutdown during the backoff sleep so cancellation
                // isn't blocked for up to MAX_BACKOFF after a fetch storm.
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    () = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    debug!(prefix = %prefix, "gateway events pull loop stopped");
}

#[cfg(coverage)]
pub async fn run_gateway_events_pull(
    _client: async_nats::Client,
    _prefix: A2aPrefix,
    _config: GatewayEventsPullConfig,
    _shutdown: CancellationToken,
) {
}

#[cfg(not(coverage))]
fn info_span_start(prefix: &A2aPrefix, durable: &EventsConsumerDurable, config: &GatewayEventsPullConfig) {
    tracing::info!(
        prefix = %prefix,
        durable = %durable.as_str(),
        max_ack_pending = config.max_ack_pending().as_usize(),
        fetch_batch = config.fetch_batch().as_usize(),
        fetch_heartbeat_secs = config.fetch_heartbeat().as_duration().as_secs(),
        max_inflight_per_caller = config.max_inflight_per_caller().as_usize(),
        "gateway events pull consumer started"
    );
}

#[cfg(not(coverage))]
async fn run_fetch_cycle(
    client: &async_nats::Client,
    prefix: &A2aPrefix,
    durable: &EventsConsumerDurable,
    planner: &BaselineTaskEventsEgressPlanner,
    config: &GatewayEventsPullConfig,
    inflight_gate: Arc<CallerInflightGate>,
    shutdown: CancellationToken,
) -> Result<(), PullCycleError> {
    let jetstream = jetstream::new(client.clone());
    let stream_name = events_stream_name(prefix);
    let stream = jetstream
        .get_stream(&stream_name)
        .await
        .map_err(|source| PullCycleError::BindStream {
            stream: stream_name.clone(),
            source,
        })?;

    let consumer_config = gateway_events_consumer(prefix, durable.as_str(), config.max_ack_pending().as_i64());
    let consumer = stream
        .get_or_create_consumer(durable.as_str(), consumer_config)
        .await
        .map_err(|source| PullCycleError::ProvisionConsumer {
            durable: durable.as_str().to_owned(),
            source,
        })?;

    let mut batch = consumer
        .fetch()
        .max_messages(config.fetch_batch().as_usize())
        .expires(FETCH_EXPIRES)
        .heartbeat(config.fetch_heartbeat().as_duration())
        .messages()
        .await
        .map_err(|source| PullCycleError::FetchMessages {
            durable: durable.as_str().to_owned(),
            source,
        })?;

    while let Some(item) = batch.next().await {
        let message = item.map_err(|source| PullCycleError::FetchItem {
            subject: String::new(),
            source,
        })?;
        let subject = message.subject.as_str();
        let Some((_task_id, req_id)) = parse_task_events_subject(prefix.as_str(), subject) else {
            message
                .ack_with(AckKind::Term)
                .await
                .map_err(|source| PullCycleError::Ack {
                    subject: subject.to_owned(),
                    source,
                })?;
            continue;
        };

        // Gate on the originating caller's identity (carried on the event
        // headers), not on `req_id`. Keying by `req_id` would let one caller
        // open many concurrent streams (distinct `req_id`s) and consume the
        // limit per stream, defeating the per-caller cap. When the header
        // is absent (legacy publisher or stripped en route) we fall back to
        // `req_id` so the gate degrades gracefully instead of opening up.
        let caller_key = caller_key_from_message_headers(&message).unwrap_or_else(|| req_id.as_str().to_string());
        let client_handle = client.clone();
        let prefix_handle = prefix.clone();
        let planner_handle = *planner;
        let subject_owned = subject.to_owned();
        let gate_handle = Arc::clone(&inflight_gate);
        let shutdown_handle = shutdown.clone();
        // Wait for a permit INSIDE the spawn so a gate-full state doesn't
        // NAK the message: NAK'ing here would tick JetStream's `delivered`
        // counter, and the planner uses that counter as the forward retry
        // budget. A caller at the inflight cap would burn the 3-attempt
        // budget on gate retries before `forward_task_event` ever ran, then
        // Term on the first publish failure. By waiting for the permit
        // before NAK'ing, `delivered` reflects actual forward attempts.
        // JetStream's `max_ack_pending` still bounds total inflight.
        tokio::spawn(async move {
            let permit = match acquire_permit_with_shutdown(gate_handle, &caller_key, &shutdown_handle).await {
                Some(permit) => permit,
                None => {
                    // Shutdown observed while waiting — NAK so the message
                    // is redelivered to the next process owner.
                    let _ = message.ack_with(AckKind::Nak(None)).await;
                    return;
                }
            };
            let _permit = permit;
            // JetStream tracks per-message delivery attempts; carry that count
            // into the planner so persistently undeliverable events get Term'd
            // after the configured retry budget instead of NAK-looping forever.
            let attempt = u32::try_from(message.info().map(|i| i.delivered).unwrap_or(1)).unwrap_or(u32::MAX);
            let forward_result = forward_task_event(&client_handle, &prefix_handle, &req_id, &message.payload).await;
            let disposition = planner_handle.forward_disposition(attempt, forward_result.err().as_deref());
            match disposition {
                EgressAckDisposition::Ack => {
                    // `double_ack` waits for the server to confirm the ack so a
                    // dropped ack doesn't trigger redelivery + duplicate publish
                    // to `gateway.egress`. On persistent ack failure we Term:
                    // the payload already shipped to the caller, and another
                    // redelivery would re-publish the same event.
                    if let Err(error) = message.double_ack().await {
                        warn!(
                            subject = %subject_owned,
                            error = %error,
                            "gateway events pull double-ack failed; terminating to avoid duplicate publish"
                        );
                        let _ = message.ack_with(AckKind::Term).await;
                    }
                }
                EgressAckDisposition::Nak { delay } => {
                    if let Err(error) = message.ack_with(AckKind::Nak(delay)).await {
                        warn!(
                            subject = %subject_owned,
                            error = %error,
                            "gateway events pull nak failed; jetstream will redeliver"
                        );
                    }
                }
                EgressAckDisposition::Term => {
                    if let Err(error) = message.ack_with(AckKind::Term).await {
                        warn!(
                            subject = %subject_owned,
                            error = %error,
                            "gateway events pull term failed; jetstream will redeliver"
                        );
                    }
                }
            }
        });
    }

    Ok(())
}

/// Block on the per-caller gate until a permit is available or shutdown
/// fires. Returns `None` only on shutdown. Yields between attempts so the
/// task doesn't spin while permits are held by other forwards.
#[cfg_attr(coverage, allow(dead_code))]
async fn acquire_permit_with_shutdown(
    gate: Arc<CallerInflightGate>,
    caller_key: &str,
    shutdown: &CancellationToken,
) -> Option<CallerInflightPermit> {
    const GATE_WAIT_BACKOFF: Duration = Duration::from_millis(50);
    loop {
        if let Some(permit) = Arc::clone(&gate).try_acquire(caller_key) {
            return Some(permit);
        }
        tokio::select! {
            () = shutdown.cancelled() => return None,
            () = tokio::time::sleep(GATE_WAIT_BACKOFF) => {}
        }
    }
}

#[cfg(not(coverage))]
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

/// Extract the originating caller identity from a JetStream task-event
/// message. Reads `X-A2a-Caller-Id` (preferred — set by the bridge ingress
/// path and propagated through the agent backend) and falls back to the
/// principal header so the per-caller inflight cap can't be circumvented
/// just by stripping one header. Returns `None` when neither is present;
/// callers should fall back to `req_id` to keep the gate non-bypassable
/// rather than opening up to unlimited concurrency on missing headers.
#[cfg(not(coverage))]
fn caller_key_from_message_headers(message: &async_nats::jetstream::Message) -> Option<String> {
    let headers = message.message.headers.as_ref()?;
    let read = |name| {
        headers
            .get(name)
            .map(|v| v.as_str().trim().to_string())
            .filter(|s| !s.is_empty())
    };
    read(a2a_nats::constants::GATEWAY_CALLER_ID_HEADER).or_else(|| read(a2a_nats::constants::GATEWAY_PRINCIPAL_HEADER))
}

#[cfg(test)]
mod tests;
