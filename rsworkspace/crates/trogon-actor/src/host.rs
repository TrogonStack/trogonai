use std::time::Duration;

use futures_util::StreamExt as _;
use tracing::{instrument, warn};
use trogon_nats::{PublishClient, RequestClient, SubscribeClient};
use trogon_registry::{AgentCapability, RegistryStore};
use trogon_transcript::TranscriptPublisher;

use crate::{EntityActor, StateStore, runtime::ActorRuntime};

/// Hosts a single `EntityActor` type: subscribes to its NATS inbox, registers
/// the actor's capabilities in the live registry, and dispatches every incoming
/// message to [`ActorRuntime::handle_event`].
///
/// ## Lifecycle
///
/// 1. On [`run`][ActorHost::run] the host registers the actor in the registry
///    and subscribes to `actors.{type}.>`.
/// 2. A background task calls [`Registry::refresh`] every
///    [`HEARTBEAT_INTERVAL`][trogon_registry::provision::HEARTBEAT_INTERVAL]
///    to keep the registry entry alive.
/// 3. For each incoming message the entity key is extracted from the subject
///    (the portion after the `actors.{type}.` prefix).
/// 4. A new actor instance is created via `Clone` and
///    `ActorRuntime::handle_event` is called.
/// 5. `run` returns when the NATS subscription closes or
///    [`cancel`][ActorHost::cancel] is called.
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
///     let publisher    = NatsTranscriptPublisher::new(js);
///     let registry     = Registry::new(reg_store.clone());
///     let runtime      = trogon_actor::ActorRuntime::new(state_store, publisher, nats, registry);
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
pub struct ActorHost<A, S, P, N, R>
where
    A: EntityActor + Clone,
    S: StateStore,
    P: TranscriptPublisher,
    N: SubscribeClient + PublishClient + RequestClient + Clone + Send + Sync + 'static,
    R: RegistryStore,
{
    runtime: ActorRuntime<S, P, N, R>,
    /// Template actor — cloned once per incoming message so each invocation
    /// gets a fresh `&mut self`.
    actor: A,
    capability: AgentCapability,
    cancel: tokio::sync::watch::Sender<bool>,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
}

impl<A, S, P, N, R> ActorHost<A, S, P, N, R>
where
    A: EntityActor + Clone,
    S: StateStore,
    P: TranscriptPublisher,
    N: SubscribeClient + PublishClient + RequestClient + Clone + Send + Sync + 'static,
    R: RegistryStore,
{
    pub fn new(
        runtime: ActorRuntime<S, P, N, R>,
        actor: A,
        capability: AgentCapability,
    ) -> Self {
        let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
        Self { runtime, actor, capability, cancel, cancel_rx }
    }

    /// Signal the host to stop after the current event (if any) completes.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    /// Register in the registry, subscribe to the actor inbox, and process
    /// events until the subscription ends or [`cancel`][Self::cancel] is called.
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
            let mut interval = tokio::time::interval(
                trogon_registry::provision::HEARTBEAT_INTERVAL,
            );
            interval.tick().await; // skip the first immediate tick
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
                    // Extract entity key from subject, e.g. "actors.pr.owner.repo.1" → "owner.repo.1"
                    let entity_key = msg.subject.as_str()
                        .strip_prefix(&key_prefix)
                        .unwrap_or(msg.subject.as_str());

                    let mut actor = self.actor.clone();
                    if let Err(e) = self.runtime.handle_event(&mut actor, entity_key).await {
                        warn!(
                            error = %e,
                            entity_key,
                            "handle_event error — continuing"
                        );
                    }
                }
                _ = cancel_rx.changed() => break,
            }
        }

        // ── Unregister on clean shutdown ──────────────────────────────────────
        if let Err(e) = registry.unregister(A::actor_type()).await {
            warn!(error = %e, "failed to unregister on shutdown");
        }

        tracing::info!(actor_type = A::actor_type(), "actor host stopped");
        Ok(())
    }
}

// ── HostError ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HostError {
    Register(String),
    Subscribe(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Register(e) => write!(f, "actor host register error: {e}"),
            HostError::Subscribe(e) => write!(f, "actor host subscribe error: {e}"),
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
            HostError::Register("x".into()).to_string().contains("register")
        );
        assert!(
            HostError::Subscribe("x".into()).to_string().contains("subscribe")
        );
    }
}
