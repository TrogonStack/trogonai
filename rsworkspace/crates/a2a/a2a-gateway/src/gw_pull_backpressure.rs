//! Gateway pull consumer for task-event egress (`A2A_EVENTS`) with
//! JetStream flow control.
//!
//! Operator guide:
//! [`../../../../../docs/a2a/how-to/operators/streaming-backpressure.md`](../../../../../docs/a2a/how-to/operators/streaming-backpressure.md).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use async_nats::jetstream::consumer::AckPolicy;
use tokio_util::sync::CancellationToken;
use trogon_std::env::ReadEnv;

use a2a_nats::jetstream::consumers::gateway_events_consumer;
use a2a_nats::jetstream::streams::events_stream_name;
use a2a_nats::nats::subjects::gateway::GatewayEgressEventsSubject;
use a2a_nats::nats::subjects::subscriptions::CallerTaskEventsSubject;
use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use async_nats::jetstream::{self, AckKind};
use futures::{Stream, StreamExt};
use tracing::{debug, warn};
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::jetstream::message::{JsAckWith, JsDoubleAck, JsMessageRef};

pub use crate::constants::{
    DEFAULT_FETCH_BATCH, DEFAULT_FETCH_HEARTBEAT_SECS, DEFAULT_INACTIVE_THRESHOLD_SECS, DEFAULT_MAX_ACK_PENDING,
    DEFAULT_MAX_INFLIGHT_PER_CALLER, ENV_GATEWAY_EVENTS_FETCH_BATCH, ENV_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS,
    ENV_GATEWAY_EVENTS_MAX_ACK_PENDING, ENV_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER, ENV_GATEWAY_EVENTS_PULL,
};
use crate::constants::{FETCH_EXPIRES, GATE_NAK_DELAY, INITIAL_BACKOFF, MAX_BACKOFF};

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
    pub start_sequence: u64,
    pub hints: PullConsumerHints,
}

pub trait TaskEventsEgressPlanner {
    type Error: std::error::Error + Send + Sync + 'static;

    fn pull_hints(&self) -> PullConsumerHints;

    /// `last_seq` is the events stream head read before the request was forwarded.
    /// The filter spans every task of the caller, so delivery has to resume past it
    /// rather than replay from the beginning of the stream.
    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        caller: &SubjectPrincipal,
        last_seq: u64,
    ) -> Result<MessageStreamEgressPlan, Self::Error>;

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        caller: &SubjectPrincipal,
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

    /// No `req_id` to narrow on: the agent answers the caller's inbox directly, so
    /// the gateway never learns the `task_id` either. The filter takes every task
    /// of the caller and the pump drops what `Trogon-Req-Id` says is not its request.
    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        caller: &SubjectPrincipal,
        last_seq: u64,
    ) -> Result<MessageStreamEgressPlan, Self::Error> {
        Ok(MessageStreamEgressPlan {
            filter_subject: CallerTaskEventsSubject::new(prefix, caller).to_string(),
            start_sequence: last_seq.saturating_add(1),
            hints: self.hints,
        })
    }

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        caller: &SubjectPrincipal,
        last_seq: u64,
    ) -> Result<ResubscribeEgressPlan, Self::Error> {
        Ok(ResubscribeEgressPlan {
            filter_subject: TaskEventsSubject::new(prefix, task_id, caller).to_string(),
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

struct CallerInflightGate {
    limit: usize,
    inflight: Mutex<HashMap<SubjectPrincipal, usize>>,
}

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
    fn try_acquire(self: Arc<Self>, caller: &SubjectPrincipal) -> Option<CallerInflightPermit> {
        // Recover from a poisoned lock — see the matching note in
        // `gw_ingress_stream::CallerInflightGate`.
        let mut guard = match self.inflight.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let count = guard.entry(caller.clone()).or_insert(0);
        if *count >= self.limit {
            return None;
        }
        *count += 1;
        drop(guard);
        Some(CallerInflightPermit {
            gate: self,
            caller: caller.clone(),
        })
    }
}

struct CallerInflightPermit {
    gate: Arc<CallerInflightGate>,
    caller: SubjectPrincipal,
}

impl Drop for CallerInflightPermit {
    fn drop(&mut self) {
        let mut guard = match self.gate.inflight.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // `try_acquire` inserted + incremented this bucket and each permit
        // decrements only its own key, so the entry is always present here.
        // Use `entry().or_insert(0)` instead of `if let Some` to avoid a
        // defensive branch that the type system already rules out.
        let count = guard.entry(self.caller.clone()).or_insert(0);
        *count = count.saturating_sub(1);
        if *count == 0 {
            guard.remove(&self.caller);
        }
    }
}

/// Typed failure surface for the pull-cycle loop. Replaces the previous
/// `Result<(), String>` so structured source errors (jetstream binding,
/// consumer provisioning, fetch, ack) survive across the loop instead of
/// being flattened into a Display string.
#[derive(Debug, thiserror::Error)]
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

/// Run the gateway egress pull-consumer loop.
pub async fn run_gateway_events_pull(
    client: async_nats::Client,
    prefix: A2aPrefix,
    config: GatewayEventsPullConfig,
    shutdown: CancellationToken,
) {
    let planner = BaselineTaskEventsEgressPlanner::new();
    let durable = EventsConsumerDurable::for_prefix(&prefix);
    let inflight_gate = Arc::new(CallerInflightGate::new(config.max_inflight_per_caller().as_usize()));
    let forward_attempts = Arc::new(ForwardAttempts::new());
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
            Arc::clone(&forward_attempts),
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

#[allow(clippy::too_many_arguments)]
async fn run_fetch_cycle(
    client: &async_nats::Client,
    prefix: &A2aPrefix,
    durable: &EventsConsumerDurable,
    planner: &BaselineTaskEventsEgressPlanner,
    config: &GatewayEventsPullConfig,
    inflight_gate: Arc<CallerInflightGate>,
    forward_attempts: Arc<ForwardAttempts>,
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

    let batch = consumer
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

    let deliveries = batch.map(|item| {
        item.map(|message| EgressDelivery {
            sequence: message.info().map(|info| info.stream_sequence).unwrap_or(0),
            message,
        })
    });
    forward_egress_deliveries(
        client,
        prefix,
        planner,
        inflight_gate,
        forward_attempts,
        shutdown,
        deliveries,
    )
    .await
}

struct EgressDelivery<M> {
    message: M,
    sequence: u64,
}

async fn forward_egress_deliveries<C, M, E>(
    client: &C,
    prefix: &A2aPrefix,
    planner: &BaselineTaskEventsEgressPlanner,
    inflight_gate: Arc<CallerInflightGate>,
    forward_attempts: Arc<ForwardAttempts>,
    shutdown: CancellationToken,
    mut batch: impl Stream<Item = Result<EgressDelivery<M>, E>> + Unpin,
) -> Result<(), PullCycleError>
where
    C: trogon_nats::PublishClient,
    M: JsMessageRef + JsAckWith + JsDoubleAck,
    <M as JsAckWith>::Error: Into<async_nats::Error>,
    E: Into<async_nats::Error>,
{
    loop {
        // Race the fetch poll against shutdown so a draining batch can't
        // keep us blocked for up to FETCH_EXPIRES (30s) after cancel, and
        // so we stop spawning new forwards once the runtime is going down.
        let item = tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            next = batch.next() => next,
        };
        let Some(item) = item else { break };
        let EgressDelivery { message, sequence } = item.map_err(|source| PullCycleError::FetchItem {
            subject: String::new(),
            source: source.into(),
        })?;
        let subject = message.message().subject.as_str();
        // Term what cannot be routed. The caller is the principal the event
        // subject names and never a header (ADR#0082); the request travels in
        // `Trogon-Req-Id` (ADR#0055). An event missing either has no egress address.
        let routing = TaskEventsSubject::parse(prefix, subject)
            .ok()
            .zip(egress_req_id_from_headers(message.message().headers.as_ref()));
        let Some((event, req_id)) = routing else {
            message
                .ack_with(AckKind::Term)
                .await
                .map_err(|source| PullCycleError::Ack {
                    subject: subject.to_owned(),
                    source: source.into(),
                })?;
            continue;
        };

        // Gate on the originating caller's identity, not on `req_id`. Keying by
        // `req_id` would let one caller open many concurrent streams (distinct
        // `req_id`s) and consume the limit per stream, defeating the per-caller
        // cap.
        let caller = event.caller().clone();
        // `try_acquire` (not wait): when the per-caller gate is full, NAK
        // immediately with a short delay so the JetStream ack-pending slot
        // is freed for other callers. Waiting inside a spawned task would
        // let a single caller fill the shared `max_ack_pending` budget with
        // permit-waiting tasks, starving every other caller. The forward
        // retry budget is tracked separately (see `forward_attempts`) so
        // gate redeliveries never tick the planner's attempt counter.
        let Some(permit) = Arc::clone(&inflight_gate).try_acquire(&caller) else {
            message
                .ack_with(AckKind::Nak(Some(GATE_NAK_DELAY)))
                .await
                .map_err(|source| PullCycleError::Ack {
                    subject: subject.to_owned(),
                    source: source.into(),
                })?;
            continue;
        };

        let client_handle = client.clone();
        let prefix_handle = prefix.clone();
        let planner_handle = *planner;
        let subject_owned = subject.to_owned();
        let attempts_handle = Arc::clone(&forward_attempts);
        // Spawn forward+ack so multiple permits per caller can coexist —
        // running this work inline serializes the loop, which would make
        // `max_inflight_per_caller` a no-op (at most one permit per caller
        // would ever exist).
        tokio::spawn(async move {
            let _permit = permit;
            let forward_result = forward_task_event(
                &client_handle,
                &prefix_handle,
                &caller,
                &req_id,
                &message.message().payload,
            )
            .await;
            let attempt = if forward_result.is_err() {
                attempts_handle.record_attempt(sequence)
            } else {
                0
            };
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
                    attempts_handle.clear(sequence);
                }
                EgressAckDisposition::Nak { delay } => {
                    if let Err(error) = message.ack_with(AckKind::Nak(delay)).await {
                        warn!(
                            subject = %subject_owned,
                            error = %error,
                            "gateway events pull nak failed; jetstream will redeliver"
                        );
                    }
                    // Keep the forward-attempt count so the next redelivery
                    // sees the same `attempt` number and the budget retires.
                }
                EgressAckDisposition::Term => {
                    if let Err(error) = message.ack_with(AckKind::Term).await {
                        warn!(
                            subject = %subject_owned,
                            error = %error,
                            "gateway events pull term failed; jetstream will redeliver"
                        );
                    }
                    attempts_handle.clear(sequence);
                }
            }
        });
    }

    Ok(())
}

/// Forward-attempt counter keyed by JetStream stream sequence.
///
/// JetStream's per-message `delivered` count ticks on every redelivery,
/// including ones we triggered ourselves by NAK'ing on gate-full. If the
/// planner used `delivered` as the forward retry budget, gate redeliveries
/// would burn the budget before `forward_task_event` ever ran — see the
/// "Gate NAKs consume forward retries" review thread. This in-process map
/// tracks attempts only when the actual publish to `gateway.egress` fails,
/// so the budget reflects real forward failures.
#[derive(Default)]
struct ForwardAttempts {
    by_sequence: Mutex<HashMap<u64, u32>>,
}

impl ForwardAttempts {
    fn new() -> Self {
        Self::default()
    }

    /// Record one failed forward attempt and return the new count.
    fn record_attempt(&self, sequence: u64) -> u32 {
        let mut guard = match self.by_sequence.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let count = guard.entry(sequence).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    /// Drop the per-sequence counter once the message reaches a terminal
    /// disposition (Ack or Term). Keeps the map's memory footprint bounded
    /// over long-lived runs.
    fn clear(&self, sequence: u64) {
        let mut guard = match self.by_sequence.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.remove(&sequence);
    }
}

/// Forward a JetStream task event to the caller's gateway-egress subject.
///
/// Generic over `trogon_nats::PublishClient` so the unit tests can drive
/// this through `MockNatsClient` without standing up a NATS server. The
/// production pump passes the concrete `async_nats::Client` which already
/// implements the trait.
async fn forward_task_event<C: trogon_nats::PublishClient>(
    client: &C,
    prefix: &A2aPrefix,
    caller: &SubjectPrincipal,
    req_id: &ReqId,
    payload: &bytes::Bytes,
) -> Result<(), String> {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(a2a_nats::constants::REQ_ID_HEADER, req_id.as_str());
    client
        .publish_with_headers(
            GatewayEgressEventsSubject::new(prefix, caller),
            headers,
            payload.clone(),
        )
        .await
        .map_err(|e| e.to_string())
}

/// The request an egressed event belongs to, read from `Trogon-Req-Id`.
///
/// The caller headers an event may still carry never choose its egress address:
/// the caller is the principal the event subject names (ADR#0082). Returns `None`
/// when the header is missing or blank, because the event then has no request to
/// egress for.
fn egress_req_id_from_headers(headers: Option<&async_nats::HeaderMap>) -> Option<ReqId> {
    let req_id = headers?
        .get(a2a_nats::constants::REQ_ID_HEADER)
        .map(|value| value.as_str().trim().to_owned())
        .filter(|value| !value.is_empty())?;
    Some(ReqId::from_header(req_id))
}

#[cfg(test)]
mod tests;
