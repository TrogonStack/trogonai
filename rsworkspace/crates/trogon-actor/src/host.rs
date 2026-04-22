use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt as _;
use tracing::{instrument, warn};
use trogon_nats::jetstream::JetStreamPublisher;
use trogon_nats::{PublishClient, RequestClient, SubscribeClient};
use trogon_registry::{AgentCapability, RegistryStore};
use trogon_transcript::TranscriptPublisher;

use crate::{
    EntityActor, StateStore,
    runtime::{ActorRuntime, SPAWN_DEPTH_HEADER, TROGON_REPLY_TO_HEADER},
    state::state_kv_key,
    telemetry::metrics,
};

/// Default maximum time allowed for a single `handle_event` invocation.
pub const DEFAULT_EVENT_TIMEOUT: Duration = Duration::from_secs(60);

/// Hosts a single `EntityActor` type: subscribes to its NATS inbox, registers
/// the actor's capabilities in the live registry, and dispatches every incoming
/// message to [`ActorRuntime::handle_event`].
///
/// After a successful `handle_event`, the host checks for a
/// [`TROGON_REPLY_TO_HEADER`] header on the incoming message.  If present, it
/// loads the actor's post-event state from KV and publishes it to that inbox
/// subject so that the spawning actor's `spawn_agent` call can unblock.
///
/// ## Example
///
/// ```rust,no_run
/// # use trogon_actor::{ActorContext, EntityActor, host::ActorHost, provision_state};
/// # use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
/// # use trogon_transcript::publisher::NatsTranscriptPublisher;
/// # #[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
/// # struct PrState { reviews: u32 }
/// # #[derive(Clone)]
/// # struct PrActor;
/// # impl EntityActor for PrActor {
/// #     type State = PrState;
/// #     type Error = std::convert::Infallible;
/// #     fn actor_type() -> &'static str { "pr" }
/// #     async fn handle(&mut self, _s: &mut PrState, _ctx: &ActorContext)
/// #         -> Result<(), Self::Error> { Ok(()) }
/// # }
/// async fn run(nats: async_nats::Client, js: async_nats::jetstream::Context) {
///     let state_store  = provision_state(&js).await.unwrap();
///     let reg_store    = provision_registry(&js).await.unwrap();
///     let publisher    = NatsTranscriptPublisher::new(js.clone());
///     let registry     = Registry::new(reg_store.clone());
///     let runtime      = trogon_actor::ActorRuntime::new(state_store, publisher, nats.clone(), registry, js.clone());
///
///     let capability = AgentCapability::new(
///         "PrActor",
///         ["code_review", "security_analysis"],
///         "actors.pr.>",
///     );
///     let host = ActorHost::new(runtime, PrActor, capability);
///     host.run().await.unwrap();
/// }
/// ```
pub struct ActorHost<A, S, P, N, R, JP>
where
    A: EntityActor + Clone,
    S: StateStore,
    P: TranscriptPublisher,
    N: SubscribeClient + PublishClient + RequestClient + Clone + Send + Sync + 'static,
    R: RegistryStore,
    JP: JetStreamPublisher,
{
    runtime: ActorRuntime<S, P, N, R, JP>,
    /// Template actor — cloned once per incoming message.
    actor: A,
    capability: AgentCapability,
    /// Maximum time allowed for a single `handle_event` invocation.
    event_timeout: Duration,
    cancel: tokio::sync::watch::Sender<bool>,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
}

impl<A, S, P, N, R, JP> ActorHost<A, S, P, N, R, JP>
where
    A: EntityActor + Clone,
    S: StateStore,
    P: TranscriptPublisher,
    N: SubscribeClient + PublishClient + RequestClient + Clone + Send + Sync + 'static,
    R: RegistryStore,
    JP: JetStreamPublisher,
{
    /// Create a host with the [`DEFAULT_EVENT_TIMEOUT`].
    pub fn new(
        runtime: ActorRuntime<S, P, N, R, JP>,
        actor: A,
        capability: AgentCapability,
    ) -> Self {
        Self::with_event_timeout(runtime, actor, capability, DEFAULT_EVENT_TIMEOUT)
    }

    /// Create a host with a custom per-event timeout.
    pub fn with_event_timeout(
        runtime: ActorRuntime<S, P, N, R, JP>,
        actor: A,
        capability: AgentCapability,
        event_timeout: Duration,
    ) -> Self {
        let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
        Self {
            runtime,
            actor,
            capability,
            event_timeout,
            cancel,
            cancel_rx,
        }
    }

    /// Signal the host to stop after the current event (if any) completes.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    /// Like [`run`][Self::run] but consumes from a durable JetStream pull
    /// consumer on the [`ACTOR_INBOX_STREAM`][crate::inbox::ACTOR_INBOX_STREAM]
    /// stream instead of a core NATS subscription.
    ///
    /// On successful `handle_event` the message is ACKed; on error or timeout
    /// it is NAKed (returned for redelivery).  If the message carries a
    /// [`TROGON_REPLY_TO_HEADER`] header the actor's saved state is published
    /// to that inbox so the spawning actor can unblock.
    #[instrument(
        skip(self, js),
        fields(actor_type = A::actor_type(), subject = %self.capability.nats_subject),
        err
    )]
    pub async fn run_durable<J>(&self, js: &J) -> Result<(), HostError>
    where
        J: trogon_nats::jetstream::JetStreamGetStream,
    {
        use async_nats::jetstream::AckKind;
        use async_nats::jetstream::consumer::pull;
        use trogon_nats::jetstream::message::{JsAck, JsAckWith, JsMessageRef};
        use trogon_nats::jetstream::{JetStreamConsumer as _, JetStreamCreateConsumer as _};

        let registry = self.runtime.registry().clone();
        registry
            .register(&self.capability)
            .await
            .map_err(|e| HostError::Register(e.to_string()))?;

        tracing::info!(
            actor_type = A::actor_type(),
            subject = %self.capability.nats_subject,
            "actor registered, starting heartbeat and durable consumer"
        );

        // ── Background heartbeat ──────────────────────────────────────────────
        let heartbeat_registry = registry.clone();
        let heartbeat_cap = self.capability.clone();
        let mut cancel_rx_hb = self.cancel_rx.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(trogon_registry::provision::HEARTBEAT_INTERVAL);
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = heartbeat_registry.refresh(&heartbeat_cap).await {
                            warn!(error = %e, "registry heartbeat failed");
                        }
                    }
                    _ = cancel_rx_hb.changed() => break,
                }
            }
        });

        // ── Create durable pull consumer ──────────────────────────────────────
        let stream = js
            .get_stream(crate::inbox::ACTOR_INBOX_STREAM)
            .await
            .map_err(|e| HostError::Durable(e.to_string()))?;

        let consumer = stream
            .create_consumer(pull::Config {
                durable_name: Some(format!("actor-{}", A::actor_type())),
                filter_subject: format!("actors.{}.>", A::actor_type()),
                ack_wait: crate::inbox::ACK_WAIT,
                max_deliver: crate::inbox::MAX_DELIVER,
                ..Default::default()
            })
            .await
            .map_err(|e| HostError::Durable(e.to_string()))?;

        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| HostError::Durable(e.to_string()))?;

        let key_prefix = format!("actors.{}.", A::actor_type());
        let mut cancel_rx = self.cancel_rx.clone();

        loop {
            tokio::select! {
                maybe_item = messages.next() => {
                    match maybe_item {
                        None => break,
                        Some(Err(e)) => {
                            warn!(error = %e, "JetStream message stream error — continuing");
                        }
                        Some(Ok(msg)) => {
                            // Extract all needed fields while holding the borrow,
                            // then release it so `msg` is free for the ACK call.
                            let nats_msg: &async_nats::Message = msg.message();
                            let entity_key = nats_msg
                                .subject
                                .as_str()
                                .strip_prefix(&key_prefix)
                                .unwrap_or(nats_msg.subject.as_str())
                                .to_string();
                            let spawn_depth: u32 = nats_msg
                                .headers
                                .as_ref()
                                .and_then(|h: &async_nats::HeaderMap| h.get(SPAWN_DEPTH_HEADER))
                                .and_then(|v| v.as_str().parse().ok())
                                .unwrap_or(0);
                            let reply_to: Option<String> = nats_msg
                                .headers
                                .as_ref()
                                .and_then(|h| h.get(TROGON_REPLY_TO_HEADER))
                                .map(|v| v.as_str().to_string());

                            let mut actor = self.actor.clone();
                            let handle_start = std::time::Instant::now();
                            let event_result = tokio::time::timeout(
                                self.event_timeout,
                                self.runtime.handle_event(&mut actor, &entity_key, spawn_depth),
                            )
                            .await;
                            let latency_ms = handle_start.elapsed().as_millis() as f64;

                            match event_result {
                                Ok(Ok(())) => {
                                    metrics::record_handle_latency(A::actor_type(), latency_ms);
                                    if let Err(e) = msg.ack().await {
                                        warn!(error = %e, "failed to ACK JetStream message");
                                    }
                                    // Send spawn reply if the caller is waiting.
                                    if let Some(reply_subject) = reply_to {
                                        send_spawn_reply(
                                            self.runtime.nats(),
                                            self.runtime.state(),
                                            A::actor_type(),
                                            &entity_key,
                                            reply_subject,
                                        )
                                        .await;
                                    }
                                }
                                Ok(Err(e)) => {
                                    metrics::inc_handle_error(A::actor_type());
                                    warn!(
                                        error = %e,
                                        entity_key = %entity_key,
                                        "handle_event error — NAKing message"
                                    );
                                    let _ = msg.ack_with(AckKind::Nak(None)).await;
                                }
                                Err(_elapsed) => {
                                    metrics::inc_handle_timeout(A::actor_type());
                                    warn!(
                                        entity_key = %entity_key,
                                        timeout_secs = self.event_timeout.as_secs(),
                                        "handle_event timed out — NAKing message"
                                    );
                                    let _ = msg.ack_with(AckKind::Nak(None)).await;
                                }
                            }
                        }
                    }
                }
                _ = cancel_rx.changed() => break,
            }
        }

        if let Err(e) = registry.unregister(A::actor_type()).await {
            warn!(error = %e, "failed to unregister on shutdown");
        }

        tracing::info!(actor_type = A::actor_type(), "actor host stopped");
        Ok(())
    }

    /// Register in the registry, subscribe to the actor inbox (core NATS), and
    /// process events until the subscription ends or [`cancel`][Self::cancel]
    /// is called.
    ///
    /// After a successful `handle_event`, the host checks for a
    /// [`TROGON_REPLY_TO_HEADER`] header and sends the actor's saved state to
    /// that inbox so the spawning actor can unblock.
    #[instrument(
        skip(self),
        fields(actor_type = A::actor_type(), subject = %self.capability.nats_subject),
        err
    )]
    pub async fn run(&self) -> Result<(), HostError> {
        let registry = self.runtime.registry().clone();
        registry
            .register(&self.capability)
            .await
            .map_err(|e| HostError::Register(e.to_string()))?;

        tracing::info!(
            actor_type = A::actor_type(),
            subject = %self.capability.nats_subject,
            "actor registered, starting heartbeat and subscription"
        );

        // ── Background heartbeat ──────────────────────────────────────────────
        let heartbeat_registry = registry.clone();
        let heartbeat_cap = self.capability.clone();
        let mut cancel_rx_hb = self.cancel_rx.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(trogon_registry::provision::HEARTBEAT_INTERVAL);
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = heartbeat_registry.refresh(&heartbeat_cap).await {
                            warn!(error = %e, "registry heartbeat failed");
                        }
                    }
                    _ = cancel_rx_hb.changed() => break,
                }
            }
        });

        // ── Subscribe to actor inbox ──────────────────────────────────────────
        let subject = self.capability.nats_subject.clone();
        let key_prefix = format!("actors.{}.", A::actor_type());

        let mut sub = self
            .runtime
            .nats()
            .subscribe(subject.clone())
            .await
            .map_err(|e| HostError::Subscribe(e.to_string()))?;

        tracing::info!(%subject, "listening for events");

        let mut cancel_rx = self.cancel_rx.clone();

        loop {
            tokio::select! {
                msg = sub.next() => {
                    let Some(msg) = msg else { break };

                    let entity_key = msg.subject.as_str()
                        .strip_prefix(&key_prefix)
                        .unwrap_or(msg.subject.as_str());
                    let spawn_depth: u32 = msg
                        .headers
                        .as_ref()
                        .and_then(|h| h.get(SPAWN_DEPTH_HEADER))
                        .and_then(|v| v.as_str().parse().ok())
                        .unwrap_or(0);
                    let reply_to: Option<String> = msg
                        .headers
                        .as_ref()
                        .and_then(|h| h.get(TROGON_REPLY_TO_HEADER))
                        .map(|v| v.as_str().to_string());

                    let mut actor = self.actor.clone();
                    let handle_start = std::time::Instant::now();
                    let event_result = tokio::time::timeout(
                        self.event_timeout,
                        self.runtime.handle_event(&mut actor, entity_key, spawn_depth),
                    )
                    .await;
                    let latency_ms = handle_start.elapsed().as_millis() as f64;
                    match event_result {
                        Ok(Ok(())) => {
                            metrics::record_handle_latency(A::actor_type(), latency_ms);
                            if let Some(reply_subject) = reply_to {
                                send_spawn_reply(
                                    self.runtime.nats(),
                                    self.runtime.state(),
                                    A::actor_type(),
                                    entity_key,
                                    reply_subject,
                                )
                                .await;
                            }
                        }
                        Ok(Err(e)) => {
                            metrics::inc_handle_error(A::actor_type());
                            warn!(
                                error = %e,
                                entity_key,
                                "handle_event error — continuing"
                            );
                        }
                        Err(_elapsed) => {
                            metrics::inc_handle_timeout(A::actor_type());
                            warn!(
                                entity_key,
                                timeout_secs = self.event_timeout.as_secs(),
                                "handle_event timed out — event dropped, continuing"
                            );
                        }
                    }
                }
                _ = cancel_rx.changed() => break,
            }
        }

        if let Err(e) = registry.unregister(A::actor_type()).await {
            warn!(error = %e, "failed to unregister on shutdown");
        }

        tracing::info!(actor_type = A::actor_type(), "actor host stopped");
        Ok(())
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Load the actor's saved state from KV and publish it to `reply_subject` so
/// that the spawning actor's inbox-wait can unblock.
///
/// Failures are logged as warnings — the spawning actor will time out and NAK
/// its own event for retry if it doesn't receive the reply in time.
async fn send_spawn_reply<N, S>(
    nats: &N,
    state: &S,
    actor_type: &str,
    entity_key: &str,
    reply_subject: String,
) where
    N: PublishClient,
    S: StateStore,
{
    let kv_key = state_kv_key(actor_type, entity_key);
    let reply_payload = match state.load(&kv_key).await {
        Ok(Some(entry)) => entry.value,
        Ok(None) => Bytes::new(),
        Err(e) => {
            warn!(error = %e, "failed to load state for spawn reply — sending empty payload");
            Bytes::new()
        }
    };
    if let Err(e) = nats
        .publish_with_headers(reply_subject, HeaderMap::new(), reply_payload)
        .await
    {
        warn!(error = %e, "failed to publish spawn reply");
    }
}

// ── HostError ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HostError {
    Register(String),
    Subscribe(String),
    /// Failed to set up the durable JetStream consumer.
    Durable(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Register(e) => write!(f, "actor host register error: {e}"),
            HostError::Subscribe(e) => write!(f, "actor host subscribe error: {e}"),
            HostError::Durable(e) => write!(f, "actor host durable consumer error: {e}"),
        }
    }
}

impl std::error::Error for HostError {}

// ── Heartbeat interval re-export ──────────────────────────────────────────────

/// How often the host sends a registry heartbeat to keep the actor visible.
pub const HEARTBEAT_INTERVAL: Duration = trogon_registry::provision::HEARTBEAT_INTERVAL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_error_display() {
        assert!(
            HostError::Register("x".into())
                .to_string()
                .contains("register")
        );
        assert!(
            HostError::Subscribe("x".into())
                .to_string()
                .contains("subscribe")
        );
    }
}
