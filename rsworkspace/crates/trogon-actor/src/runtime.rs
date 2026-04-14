use std::sync::Arc;

use async_nats::HeaderMap;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use tracing::instrument;
use trogon_nats::RequestClient;
use trogon_registry::{Registry, RegistryStore};
use trogon_transcript::{Session, TranscriptEntry, TranscriptPublisher, entry::now_ms};

use crate::{
    actor::EntityActor,
    context::ActorContext,
    error::{ActorError, SaveError},
    state::{MAX_OCC_RETRIES, StateStore, state_kv_key},
};

/// Orchestrates a single event invocation for an [`EntityActor`].
///
/// The runtime is generic over the infrastructure components but the actor
/// implementor never interacts with it directly — they only implement
/// [`EntityActor`] and use [`ActorContext`].
///
/// ## Type parameters
///
/// | Param | Role |
/// |---|---|
/// | `S` | State store (NATS KV in production, `MockStateStore` in tests) |
/// | `P` | Transcript publisher (`NatsTranscriptPublisher` / `MockTranscriptPublisher`) |
/// | `N` | NATS client for sub-agent request-reply |
/// | `R` | Registry store for discovering sub-agents |
#[derive(Clone)]
pub struct ActorRuntime<S, P, N, R>
where
    S: StateStore,
    P: TranscriptPublisher,
    N: RequestClient,
    R: RegistryStore,
{
    state: S,
    publisher: P,
    nats: N,
    registry: Registry<R>,
}

impl<S, P, N, R> ActorRuntime<S, P, N, R>
where
    S: StateStore,
    P: TranscriptPublisher,
    N: RequestClient,
    R: RegistryStore,
{
    pub fn new(state: S, publisher: P, nats: N, registry: Registry<R>) -> Self {
        Self { state, publisher, nats, registry }
    }

    /// Run the full event-handling cycle for one actor invocation.
    ///
    /// The cycle is:
    /// 1. Load entity state from KV (call `on_create` if first event).
    /// 2. Open a transcript session.
    /// 3. Call `actor.handle(state, ctx)`.
    /// 4. Save updated state with optimistic concurrency.
    /// 5. On conflict: reload state and retry (up to [`MAX_OCC_RETRIES`] times).
    #[instrument(
        skip(self, actor),
        fields(
            actor_type = A::actor_type(),
            entity_key,
        ),
        err
    )]
    pub async fn handle_event<A>(
        &self,
        actor: &mut A,
        entity_key: &str,
    ) -> Result<(), ActorError<A::Error>>
    where
        A: EntityActor,
    {
        let kv_key = state_kv_key(A::actor_type(), entity_key);

        for attempt in 0..=MAX_OCC_RETRIES {
            if attempt > 0 {
                tracing::warn!(attempt, entity_key, "optimistic concurrency conflict — retrying");
            }

            // ── 1. Load state ─────────────────────────────────────────────────
            let (mut state, revision) = match self
                .state
                .load(&kv_key)
                .await
                .map_err(ActorError::State)?
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
            let ctx = build_context::<A, P, N, R>(
                entity_key,
                session_id,
                Arc::clone(&session),
                self.nats.clone(),
                self.registry.clone(),
            );

            // ── 4. Handle event ───────────────────────────────────────────────
            actor.handle(&mut state, &ctx).await.map_err(ActorError::Actor)?;

            // ── 5. Save state with OCC ────────────────────────────────────────
            let bytes = serde_json::to_vec(&state).map_err(ActorError::Serialize)?;
            match self
                .state
                .save(&kv_key, Bytes::from(bytes), revision)
                .await
            {
                Ok(_) => return Ok(()),
                Err(SaveError::Conflict) => continue,
                Err(SaveError::Other(msg)) => return Err(ActorError::State(msg)),
            }
        }

        Err(ActorError::RetryLimitExceeded)
    }
}

// ── Context construction ──────────────────────────────────────────────────────

fn build_context<A, P, N, R>(
    entity_key: &str,
    session_id: String,
    session: Arc<Session<P>>,
    nats: N,
    registry: Registry<R>,
) -> ActorContext
where
    A: EntityActor,
    P: TranscriptPublisher,
    N: RequestClient,
    R: RegistryStore,
{
    let parent_id = format!("{}/{}", A::actor_type(), entity_key);
    let session_for_spawn = Arc::clone(&session);

    // Append function: delegates to the session, converts error to String.
    let append_fn: Arc<dyn Fn(TranscriptEntry) -> BoxFuture<'static, Result<(), String>> + Send + Sync> = {
        Arc::new(move |entry| {
            let s = Arc::clone(&session);
            Box::pin(async move {
                s.append(entry).await.map_err(|e| e.to_string())
            })
        })
    };

    // Spawn function: registry lookup → request-reply → transcript entry.
    let spawn_fn: Arc<dyn Fn(String, Bytes) -> BoxFuture<'static, Result<Bytes, String>> + Send + Sync> = {
        Arc::new(move |capability: String, payload: Bytes| {
            let reg = registry.clone();
            let nats = nats.clone();
            let parent = parent_id.clone();
            let session = Arc::clone(&session_for_spawn);

            Box::pin(async move {
                // Find the first agent with the requested capability.
                let agents = reg
                    .discover(&capability)
                    .await
                    .map_err(|e| e.to_string())?;

                let agent = agents.first().ok_or_else(|| {
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

                // Send request to the sub-agent's inbox and await reply.
                let reply = nats
                    .request_with_headers(subject, HeaderMap::new(), payload)
                    .await
                    .map_err(|e| format!("sub-agent request failed: {e}"))?;

                Ok(reply.payload)
            })
        })
    };

    ActorContext::new(entity_key, A::actor_type(), session_id, append_fn, spawn_fn)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use trogon_registry::MockRegistryStore;
    use trogon_transcript::publisher::mock::MockTranscriptPublisher;

    use crate::{
        actor::EntityActor,
        context::ActorContext,
        state::mock::MockStateStore,
    };

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
            (Self { handled: Arc::clone(&handled) }, handled)
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
    > {
        let state = MockStateStore::new();
        let publisher = MockTranscriptPublisher::new();
        let nats = trogon_nats::MockNatsClient::new();
        let registry = Registry::new(MockRegistryStore::new());
        ActorRuntime::new(state, publisher, nats, registry)
    }

    // ── Basic flow ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn first_event_initialises_state() {
        let runtime = make_runtime();
        let (mut actor, handled) = CounterActor::new();

        runtime
            .handle_event(&mut actor, "entity-1")
            .await
            .unwrap();

        assert_eq!(*handled.lock().unwrap(), 1);

        // State should be persisted.
        let kv_key = state_kv_key("counter", "entity-1");
        let entry = runtime.state.load(&kv_key).await.unwrap().unwrap();
        let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
        assert_eq!(saved.count, 1);
    }

    #[tokio::test]
    async fn second_event_accumulates_state() {
        let runtime = make_runtime();
        let (mut actor, _) = CounterActor::new();

        runtime.handle_event(&mut actor, "entity-1").await.unwrap();
        runtime.handle_event(&mut actor, "entity-1").await.unwrap();

        let kv_key = state_kv_key("counter", "entity-1");
        let entry = runtime.state.load(&kv_key).await.unwrap().unwrap();
        let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
        assert_eq!(saved.count, 2);
    }

    #[tokio::test]
    async fn retries_on_occ_conflict() {
        let runtime = make_runtime();
        // Inject 2 conflicts — the runtime should retry and eventually succeed.
        runtime.state.inject_conflicts(2);

        let (mut actor, handled) = CounterActor::new();
        runtime.handle_event(&mut actor, "entity-1").await.unwrap();

        // handle() was called 3 times (2 retried + 1 success).
        assert_eq!(*handled.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn exceeds_retry_limit_returns_error() {
        let runtime = make_runtime();
        runtime.state.inject_conflicts(MAX_OCC_RETRIES + 1);

        let (mut actor, _) = CounterActor::new();
        let err = runtime
            .handle_event(&mut actor, "entity-1")
            .await
            .unwrap_err();

        assert!(matches!(err, ActorError::RetryLimitExceeded));
    }

    #[tokio::test]
    async fn save_other_error_propagates_as_actor_error_state() {
        let runtime = make_runtime();
        runtime.state.inject_save_error();
        let (mut actor, _) = CounterActor::new();
        let err = runtime.handle_event(&mut actor, "entity-1").await.unwrap_err();
        assert!(matches!(err, ActorError::State(_)));
    }

    #[tokio::test]
    async fn load_error_propagates_as_actor_error_state() {
        let runtime = make_runtime();
        runtime.state.inject_load_error();
        let (mut actor, _) = CounterActor::new();
        let err = runtime.handle_event(&mut actor, "entity-1").await.unwrap_err();
        assert!(matches!(err, ActorError::State(_)));
    }

    #[tokio::test]
    async fn transcript_entries_are_published() {
        // Actor that writes transcript entries.
        struct Scribe;
        impl EntityActor for Scribe {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str { "scribe" }
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
        let runtime = ActorRuntime::new(state, publisher.clone(), nats, registry);

        runtime.handle_event(&mut Scribe, "e-1").await.unwrap();

        let published = publisher.take_published();
        assert_eq!(published.len(), 2);
    }

    // ── spawn_fn closure coverage ─────────────────────────────────────────────

    #[tokio::test]
    async fn spawn_agent_through_runtime_succeeds() {
        use trogon_nats::AdvancedMockNatsClient;
        use trogon_registry::AgentCapability;

        let nats = AdvancedMockNatsClient::new();
        nats.set_response("sub.inbox", bytes::Bytes::from("pong"));

        let store = MockRegistryStore::new();
        let registry = Registry::new(store.clone());
        registry
            .register(&AgentCapability::new("SubAgent", ["analyze"], "sub.inbox"))
            .await
            .unwrap();

        let state_store = MockStateStore::new();
        let publisher = MockTranscriptPublisher::new();
        let runtime = ActorRuntime::new(state_store, publisher.clone(), nats, registry);

        struct Spawner;
        impl EntityActor for Spawner {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str { "spawner" }
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

        runtime.handle_event(&mut Spawner, "e-1").await.unwrap();

        // SubAgentSpawn entry recorded in transcript.
        let published = publisher.take_published();
        assert_eq!(published.len(), 1);
    }

    #[tokio::test]
    async fn spawn_agent_no_capability_returns_spawn_failed() {
        use trogon_nats::AdvancedMockNatsClient;

        let nats = AdvancedMockNatsClient::new();
        let registry = Registry::new(MockRegistryStore::new());
        // No agents registered.

        let runtime = ActorRuntime::new(
            MockStateStore::new(),
            MockTranscriptPublisher::new(),
            nats,
            registry,
        );

        struct NoCapActor;
        impl EntityActor for NoCapActor {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str { "no-cap" }
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

        runtime.handle_event(&mut NoCapActor, "e-1").await.unwrap();
    }

    #[tokio::test]
    async fn spawn_agent_request_failure_returns_spawn_failed() {
        use trogon_nats::AdvancedMockNatsClient;
        use trogon_registry::AgentCapability;

        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();

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
        );

        struct ReqFailActor;
        impl EntityActor for ReqFailActor {
            type State = Counter;
            type Error = std::convert::Infallible;
            fn actor_type() -> &'static str { "req-fail" }
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

        runtime.handle_event(&mut ReqFailActor, "e-1").await.unwrap();
    }
}
