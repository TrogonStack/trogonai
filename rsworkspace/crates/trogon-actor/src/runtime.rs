use std::sync::Arc;
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt as _;
use futures_util::future::BoxFuture;
use tracing::instrument;
use trogon_nats::SubscribeClient;
use trogon_nats::jetstream::JetStreamPublisher;
use trogon_registry::{Registry, RegistryStore};
use trogon_transcript::{Session, TranscriptEntry, TranscriptPublisher, entry::now_ms};

/// Maximum time to wait for a sub-agent to respond to a spawn request.
pub const SPAWN_AGENT_TIMEOUT: Duration = Duration::from_secs(30);

/// NATS header name used to carry the reply-inbox subject for JetStream spawn
/// requests.  The spawning actor subscribes to the inbox before publishing and
/// waits for the sub-agent's `ActorHost` to post the reply after handling.
pub const TROGON_REPLY_TO_HEADER: &str = "Trogon-Reply-To";

use crate::{
    actor::EntityActor,
    context::ActorContext,
    error::{ActorError, SaveError},
    state::{MAX_OCC_RETRIES, StateStore, state_kv_key},
    telemetry::metrics,
};

type SpawnFn =
    Arc<dyn Fn(String, Bytes, u32) -> BoxFuture<'static, Result<Bytes, String>> + Send + Sync>;

/// Orchestrates a single event invocation for an [`EntityActor`].
///
/// ## Type parameters
///
/// | Param | Role |
/// |---|---|
/// | `S` | State store (NATS KV in production, `MockStateStore` in tests) |
/// | `P` | Transcript publisher (`NatsTranscriptPublisher` / `MockTranscriptPublisher`) |
/// | `N` | NATS subscribe client for sub-agent reply inbox |
/// | `R` | Registry store for discovering sub-agents |
/// | `J` | JetStream publisher for durable sub-agent spawn requests |
#[derive(Clone)]
pub struct ActorRuntime<S, P, N, R, J>
where
    S: StateStore,
    P: TranscriptPublisher,
    N: SubscribeClient,
    R: RegistryStore,
    J: JetStreamPublisher,
{
    state: S,
    publisher: P,
    nats: N,
    registry: Registry<R>,
    js: J,
}

impl<S, P, N, R, J> ActorRuntime<S, P, N, R, J>
where
    S: StateStore,
    P: TranscriptPublisher,
    N: SubscribeClient,
    R: RegistryStore,
    J: JetStreamPublisher,
{
    pub fn new(state: S, publisher: P, nats: N, registry: Registry<R>, js: J) -> Self {
        Self {
            state,
            publisher,
            nats,
            registry,
            js,
        }
    }

    /// Borrow the registry — used by [`crate::host::ActorHost`] for registration
    /// and heartbeat calls.
    pub fn registry(&self) -> &Registry<R> {
        &self.registry
    }

    /// Borrow the NATS client — used by [`crate::host::ActorHost`] to subscribe
    /// to the actor inbox and to send spawn replies.
    pub fn nats(&self) -> &N {
        &self.nats
    }

    /// Borrow the state store — used by [`crate::host::ActorHost`] to load the
    /// saved state after `handle_event` so it can be forwarded as a spawn reply.
    pub fn state(&self) -> &S {
        &self.state
    }

    #[instrument(
        skip(self, actor),
        fields(
            actor_type = A::actor_type(),
            entity_key,
            spawn_depth,
        ),
        err
    )]
    pub async fn handle_event<A>(
        &self,
        actor: &mut A,
        entity_key: &str,
        spawn_depth: u32,
    ) -> Result<(), ActorError<A::Error>>
    where
        A: EntityActor,
    {
        let kv_key = state_kv_key(A::actor_type(), entity_key);

        for attempt in 0..=MAX_OCC_RETRIES {
            if attempt > 0 {
                tracing::warn!(
                    attempt,
                    entity_key,
                    "optimistic concurrency conflict — retrying"
                );
                metrics::inc_occ_retry(A::actor_type());
            }

            // ── 1. Load state ─────────────────────────────────────────────────
            let (mut state, revision) = match self
                .state
                .load(&kv_key)
                .await
                .map_err(|e| ActorError::State(e.to_string()))?
            {
                Some(entry) => {
                    let s = serde_json::from_slice::<A::State>(&entry.value)
                        .map_err(ActorError::Deserialize)?;
                    (s, Some(entry.revision))
                }
                None => {
                    let mut s = A::State::default();
                    A::on_create(&mut s).await.map_err(ActorError::Actor)?;
                    (s, None)
                }
            };

            // ── 2. Open transcript session ────────────────────────────────────
            let session = Arc::new(Session::new(
                self.publisher.clone(),
                A::actor_type(),
                entity_key,
            ));
            let session_id = session.id().to_string();

            // ── 3. Build ActorContext (type-erased closures) ──────────────────
            let ctx = build_context::<A, P, N, R, J>(
                entity_key,
                session_id,
                Arc::clone(&session),
                self.nats.clone(),
                self.registry.clone(),
                self.js.clone(),
                spawn_depth,
            );

            // ── 4. Handle event ───────────────────────────────────────────────
            actor
                .handle(&mut state, &ctx)
                .await
                .map_err(ActorError::Actor)?;

            // ── 5. Save state with OCC ────────────────────────────────────────
            let bytes = serde_json::to_vec(&state).map_err(ActorError::Serialize)?;
            match self.state.save(&kv_key, Bytes::from(bytes), revision).await {
                Ok(_) => return Ok(()),
                Err(SaveError::Conflict) => continue,
                Err(SaveError::Other(e)) => return Err(ActorError::State(e.to_string())),
            }
        }

        Err(ActorError::RetryLimitExceeded)
    }
}

// ── Context construction ──────────────────────────────────────────────────────

/// NATS header name used to propagate spawn depth across actor boundaries.
pub(crate) const SPAWN_DEPTH_HEADER: &str = "Trogon-Spawn-Depth";

fn build_context<A, P, N, R, J>(
    entity_key: &str,
    session_id: String,
    session: Arc<Session<P>>,
    nats: N,
    registry: Registry<R>,
    js: J,
    spawn_depth: u32,
) -> ActorContext
where
    A: EntityActor,
    P: TranscriptPublisher,
    N: SubscribeClient,
    R: RegistryStore,
    J: JetStreamPublisher,
{
    let parent_id = format!("{}/{}", A::actor_type(), entity_key);
    let session_for_spawn = Arc::clone(&session);

    // Append function: delegates to the session, converts error to String.
    let append_fn: Arc<
        dyn Fn(TranscriptEntry) -> BoxFuture<'static, Result<(), String>> + Send + Sync,
    > = {
        Arc::new(move |entry| {
            let s = Arc::clone(&session);
            Box::pin(async move { s.append(entry).await.map_err(|e| e.to_string()) })
        })
    };

    // Spawn function: registry lookup → JetStream publish → inbox reply → transcript.
    let actor_type_str = A::actor_type();
    let spawn_fn: SpawnFn = {
        Arc::new(move |capability: String, payload: Bytes, next_depth: u32| {
            let reg = registry.clone();
            let nats = nats.clone();
            let js = js.clone();
            let parent = parent_id.clone();
            let session = Arc::clone(&session_for_spawn);
            let actor_type = actor_type_str;

            Box::pin(async move {
                metrics::inc_spawn_call(actor_type, &capability);

                // Find the first agent with the requested capability.
                let agents = reg.discover(&capability).await.map_err(|e| {
                    metrics::inc_spawn_error(actor_type, &capability);
                    e.to_string()
                })?;

                let agent = agents.first().ok_or_else(|| {
                    metrics::inc_spawn_error(actor_type, &capability);
                    format!("no agent found with capability '{capability}'")
                })?;

                let subject = agent.nats_subject.clone();
                let child = agent.agent_type.clone();

                // Record the spawn in the transcript before sending.
                let _ = session
                    .append(TranscriptEntry::SubAgentSpawn {
                        parent: parent.clone(),
                        child: child.clone(),
                        capability: capability.clone(),
                        timestamp: now_ms(),
                    })
                    .await;

                // Build request headers: spawn depth + reply inbox.
                let mut headers = HeaderMap::new();
                headers.insert(SPAWN_DEPTH_HEADER, next_depth.to_string().as_str());

                // Subscribe to a unique inbox *before* publishing so we never
                // miss the reply.
                let inbox = format!("_INBOX.spawn.{}", uuid::Uuid::new_v4());
                headers.insert(TROGON_REPLY_TO_HEADER, inbox.as_str());

                let mut reply_sub = nats.subscribe(inbox).await.map_err(|e| {
                    metrics::inc_spawn_error(actor_type, &capability);
                    format!("failed to subscribe to spawn reply inbox: {e}")
                })?;

                // Publish via JetStream for durable delivery; discard the
                // AckFuture (fire-and-forget ACK) to avoid lifetime complexity
                // inside BoxFuture<'static>.  The sub-agent's reply serves as
                // the implicit confirmation that the message was processed.
                js.publish_with_headers(subject, headers, payload)
                    .await
                    .map_err(|e| {
                        metrics::inc_spawn_error(actor_type, &capability);
                        format!("JetStream spawn publish failed: {e}")
                    })?;

                // Wait for the sub-agent to post its reply on the inbox.
                let reply = tokio::time::timeout(SPAWN_AGENT_TIMEOUT, reply_sub.next())
                    .await
                    .map_err(|_| {
                        metrics::inc_spawn_error(actor_type, &capability);
                        format!("sub-agent request timed out after {SPAWN_AGENT_TIMEOUT:?}")
                    })?
                    .ok_or_else(|| {
                        metrics::inc_spawn_error(actor_type, &capability);
                        "spawn inbox subscription closed before reply arrived".to_string()
                    })?;

                Ok(reply.payload)
            })
        })
    };

    ActorContext::new(
        entity_key,
        A::actor_type(),
        session_id,
        spawn_depth,
        append_fn,
        spawn_fn,
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use trogon_nats::jetstream::MockJetStreamPublisher;
    use trogon_registry::MockRegistryStore;
    use trogon_transcript::publisher::mock::MockTranscriptPublisher;

    use crate::{actor::EntityActor, context::ActorContext, state::mock::MockStateStore};

    // ── Test actor ────────────────────────────────────────────────────────────

    #[derive(Default, serde::Serialize, serde::Deserialize)]
    struct Counter {
        count: u32,
    }

    struct CounterActor {
        handled: Arc<Mutex<u32>>,
    }

    impl CounterActor {
        fn new() -> (Self, Arc<Mutex<u32>>) {
            let handled = Arc::new(Mutex::new(0u32));
            (
                Self {
                    handled: Arc::clone(&handled),
                },
                handled,
            )
        }
    }

    impl EntityActor for CounterActor {
        type State = Counter;
        type Error = std::convert::Infallible;

        fn actor_type() -> &'static str {
            "counter"
        }

        async fn handle(
            &mut self,
            state: &mut Counter,
            _ctx: &ActorContext,
        ) -> Result<(), Self::Error> {
            state.count += 1;
            *self.handled.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn make_runtime() -> ActorRuntime<
        MockStateStore,
        MockTranscriptPublisher,
        trogon_nats::MockNatsClient,
        MockRegistryStore,
        MockJetStreamPublisher,
    > {
        let state = MockStateStore::new();
        let publisher = MockTranscriptPublisher::new();
        let nats = trogon_nats::MockNatsClient::new();
        let registry = Registry::new(MockRegistryStore::new());
        let js = MockJetStreamPublisher::new();
        ActorRuntime::new(state, publisher, nats, registry, js)
    }

    // ── Basic flow ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn first_event_initialises_state() {
        let runtime = make_runtime();
        let (mut actor, handled) = CounterActor::new();

        runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap();

        assert_eq!(*handled.lock().unwrap(), 1);

        let kv_key = state_kv_key("counter", "entity-1");
        let entry = runtime.state.load(&kv_key).await.unwrap().unwrap();
        let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
        assert_eq!(saved.count, 1);
    }

    #[tokio::test]
    async fn second_event_accumulates_state() {
        let runtime = make_runtime();
        let (mut actor, _) = CounterActor::new();

        runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap();
        runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap();

        let kv_key = state_kv_key("counter", "entity-1");
        let entry = runtime.state.load(&kv_key).await.unwrap().unwrap();
        let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
        assert_eq!(saved.count, 2);
    }

    #[tokio::test]
    async fn retries_on_occ_conflict() {
        let runtime = make_runtime();
        runtime.state.inject_conflicts(2);

        let (mut actor, handled) = CounterActor::new();
        runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap();

        assert_eq!(*handled.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn exceeds_retry_limit_returns_error() {
        let runtime = make_runtime();
        runtime.state.inject_conflicts(MAX_OCC_RETRIES + 1);

        let (mut actor, _) = CounterActor::new();
        let err = runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap_err();

        assert!(matches!(err, ActorError::RetryLimitExceeded));
    }

    #[tokio::test]
    async fn save_other_error_propagates_as_actor_error_state() {
        let runtime = make_runtime();
        runtime.state.inject_save_error();
        let (mut actor, _) = CounterActor::new();
        let err = runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap_err();
        assert!(matches!(err, ActorError::State(_)));
    }

    #[tokio::test]
    async fn load_error_propagates_as_actor_error_state() {
        let runtime = make_runtime();
        runtime.state.inject_load_error();
        let (mut actor, _) = CounterActor::new();
        let err = runtime
            .handle_event(&mut actor, "entity-1", 0)
            .await
            .unwrap_err();
        assert!(matches!(err, ActorError::State(_)));
    }

    #[tokio::test]
    async fn transcript_entries_are_published() {
        struct Scribe;
        impl EntityActor for Scribe {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str {
                "scribe"
            }
            async fn handle(
                &mut self,
                _state: &mut Counter,
                ctx: &ActorContext,
            ) -> Result<(), Self::Error> {
                ctx.append_user_message("hello", None).await.ok();
                ctx.append_assistant_message("hi", Some(5)).await.ok();
                Ok(())
            }
        }

        let state = MockStateStore::new();
        let publisher = MockTranscriptPublisher::new();
        let nats = trogon_nats::MockNatsClient::new();
        let registry = Registry::new(MockRegistryStore::new());
        let js = MockJetStreamPublisher::new();
        let runtime = ActorRuntime::new(state, publisher.clone(), nats, registry, js);

        runtime.handle_event(&mut Scribe, "e-1", 0).await.unwrap();

        let published = publisher.take_published();
        assert_eq!(published.len(), 2);
    }

    // ── spawn_fn closure coverage ─────────────────────────────────────────────

    /// Successful spawn: agent discovered, JetStream publish succeeds, inbox
    /// receives reply.
    #[tokio::test]
    async fn spawn_agent_through_runtime_succeeds() {
        use trogon_registry::AgentCapability;

        let nats = trogon_nats::MockNatsClient::new();
        let js = MockJetStreamPublisher::new();

        // Pre-inject a channel for the inbox subscription.  MockNatsClient
        // pops from the queue on subscribe(), so the first subscribe call
        // (from spawn_fn) gets this stream.
        let tx = nats.inject_messages();

        // After a short delay, inject the reply message so the spawn_fn's
        // inbox-wait completes.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = tx.unbounded_send(async_nats::Message {
                subject: "reply".into(),
                reply: None,
                payload: bytes::Bytes::from_static(b"pong"),
                headers: None,
                length: 4,
                status: None,
                description: None,
            });
        });

        let store = MockRegistryStore::new();
        let registry = Registry::new(store.clone());
        registry
            .register(&AgentCapability::new("SubAgent", ["analyze"], "sub.inbox"))
            .await
            .unwrap();

        let state_store = MockStateStore::new();
        let publisher = MockTranscriptPublisher::new();
        let runtime = ActorRuntime::new(state_store, publisher.clone(), nats, registry, js);

        struct Spawner;
        impl EntityActor for Spawner {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str {
                "spawner"
            }
            async fn handle(
                &mut self,
                _state: &mut Counter,
                ctx: &ActorContext,
            ) -> Result<(), Self::Error> {
                let reply = ctx
                    .spawn_agent::<Self::Error>("analyze", bytes::Bytes::new())
                    .await
                    .unwrap();
                assert_eq!(reply.as_ref(), b"pong");
                Ok(())
            }
        }

        runtime.handle_event(&mut Spawner, "e-1", 0).await.unwrap();

        // SubAgentSpawn entry recorded in transcript.
        let published = publisher.take_published();
        assert_eq!(published.len(), 1);
    }

    /// No capability registered → spawn_agent fails with no-agent error.
    #[tokio::test]
    async fn spawn_agent_no_capability_returns_spawn_failed() {
        let nats = trogon_nats::MockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let registry = Registry::new(MockRegistryStore::new());

        let runtime = ActorRuntime::new(
            MockStateStore::new(),
            MockTranscriptPublisher::new(),
            nats,
            registry,
            js,
        );

        struct NoCapActor;
        impl EntityActor for NoCapActor {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str {
                "no-cap"
            }
            async fn handle(
                &mut self,
                _state: &mut Counter,
                ctx: &ActorContext,
            ) -> Result<(), Self::Error> {
                let result = ctx
                    .spawn_agent::<Self::Error>("missing_cap", bytes::Bytes::new())
                    .await;
                assert!(result.is_err());
                Ok(())
            }
        }

        runtime
            .handle_event(&mut NoCapActor, "e-1", 0)
            .await
            .unwrap();
    }

    /// Transport failure (subscribe returns error because no stream is
    /// pre-injected) → spawn_agent fails gracefully.
    #[tokio::test]
    async fn spawn_agent_request_failure_returns_spawn_failed() {
        use trogon_registry::AgentCapability;

        // No stream pre-injected → MockNatsClient.subscribe() returns an error,
        // which simulates a transport failure in the spawn path.
        let nats = trogon_nats::MockNatsClient::new();
        let js = MockJetStreamPublisher::new();

        let store = MockRegistryStore::new();
        let registry = Registry::new(store.clone());
        registry
            .register(&AgentCapability::new("SubAgent", ["analyze"], "sub.inbox"))
            .await
            .unwrap();

        let runtime = ActorRuntime::new(
            MockStateStore::new(),
            MockTranscriptPublisher::new(),
            nats,
            registry,
            js,
        );

        struct ReqFailActor;
        impl EntityActor for ReqFailActor {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str {
                "req-fail"
            }
            async fn handle(
                &mut self,
                _state: &mut Counter,
                ctx: &ActorContext,
            ) -> Result<(), Self::Error> {
                let result = ctx
                    .spawn_agent::<Self::Error>("analyze", bytes::Bytes::new())
                    .await;
                assert!(result.is_err());
                Ok(())
            }
        }

        runtime
            .handle_event(&mut ReqFailActor, "e-1", 0)
            .await
            .unwrap();
    }
}
