use async_nats::HeaderMap;
use futures_util::StreamExt;
use tracing::{instrument, warn};
use trogon_nats::{PublishClient, SubscribeClient};
use trogon_registry::{AgentCapability, Registry, RegistryStore};
use trogon_transcript::{Session, TranscriptPublisher, entry::now_ms};

use crate::{
    decision::{RouteResult, RoutingDecision},
    error::RouterError,
    event::RouterEvent,
    llm::LlmClient,
    metrics,
    prompt::build_routing_prompt,
};

/// The Router Agent.
///
/// Subscribes to `trogon.events.>`, calls the LLM for each event to determine
/// which Entity Actor should handle it, records the routing decision in the
/// transcript, then forwards the event payload to the actor's NATS subject.
///
/// ## Type parameters
///
/// | Param | Role |
/// |---|---|
/// | `L` | LLM client (`OpenAiCompatClient` in prod, `MockLlmClient` in tests) |
/// | `R` | Registry store (`kv::Store` in prod, `MockRegistryStore` in tests) |
/// | `P` | Transcript publisher (`NatsTranscriptPublisher` / `MockTranscriptPublisher`) |
/// | `N` | NATS client (`async_nats::Client` in prod, `MockNatsClient` in tests) |
#[derive(Clone)]
pub struct Router<L, R, P, N>
where
    L: LlmClient,
    R: RegistryStore,
    P: TranscriptPublisher,
    N: SubscribeClient + PublishClient,
{
    llm: L,
    registry: Registry<R>,
    publisher: P,
    nats: N,
    /// When `Some`, unroutable events are published to
    /// `{dlq_subject_prefix}.{event_type}` in addition to being logged.
    dlq_subject_prefix: Option<String>,
}

impl<L, R, P, N> Router<L, R, P, N>
where
    L: LlmClient,
    R: RegistryStore,
    P: TranscriptPublisher,
    N: SubscribeClient + PublishClient,
{
    pub fn new(llm: L, registry: Registry<R>, publisher: P, nats: N) -> Self {
        Self { llm, registry, publisher, nats, dlq_subject_prefix: None }
    }

    /// Enable the dead-letter queue.
    ///
    /// When set, every unroutable event is published to
    /// `{subject_prefix}.{event_type}` (e.g. `trogon.unroutable.github.pull_request`).
    /// The caller is responsible for provisioning the backing JetStream stream
    /// via [`crate::unroutable::provision_unroutable_stream`].
    pub fn with_dlq(mut self, subject_prefix: impl Into<String>) -> Self {
        self.dlq_subject_prefix = Some(subject_prefix.into());
        self
    }

    /// Subscribe to `trogon.events.>` and route events until the subscription
    /// ends or an unrecoverable error occurs.
    #[instrument(skip(self), err)]
    pub async fn run(&self, subject: &str) -> Result<(), RouterError> {
        let mut sub = self
            .nats
            .subscribe(subject.to_string())
            .await
            .map_err(|e| RouterError::Subscribe(e.to_string()))?;

        tracing::info!(subject, "Router listening for events");

        while let Some(msg) = sub.next().await {
            let event = RouterEvent::from(msg);
            if let Err(e) = self.route_event(event).await {
                warn!(error = %e, "error routing event — continuing");
            }
        }

        Ok(())
    }

    /// Process a single incoming event: ask the LLM, record the decision, and
    /// forward the payload to the chosen actor's NATS subject.
    #[instrument(
        skip(self, event),
        fields(event_type = event.event_type()),
        err
    )]
    pub async fn route_event(&self, event: RouterEvent) -> Result<RouteResult, RouterError> {
        // ── 1. Fetch registered agents ────────────────────────────────────────
        let agents = self
            .registry
            .list_all()
            .await
            .map_err(|e| RouterError::Registry(e.to_string()))?;

        // ── 2. Ask the LLM ────────────────────────────────────────────────────
        let prompt = build_routing_prompt(&event, &agents);
        let llm_start = std::time::Instant::now();
        let llm_response = self.llm.complete(prompt).await.inspect_err(|_| {
            metrics::inc_events_error(event.event_type(), "llm");
        })?;
        metrics::record_llm_latency(
            event.event_type(),
            llm_start.elapsed().as_millis() as f64,
        );
        let result: RouteResult = llm_response.into();

        // ── 3. Record routing decision in transcript (best-effort) ────────────
        let session = Session::new(self.publisher.clone(), "router", &event.subject);
        match &result {
            RouteResult::Routed(decision) => {
                if let Err(e) = session
                    .append_routing_decision(
                        event.event_type(),
                        &decision.agent_type,
                        &decision.reasoning,
                    )
                    .await
                {
                    warn!(error = %e, "failed to write routing decision to transcript");
                }
                // ── 4. Forward to actor ───────────────────────────────────────
                // Pass the already-fetched agent list so dispatch() does not
                // perform a second registry lookup (eliminates TOCTOU race).
                self.dispatch(&event, decision, &agents).await.inspect_err(|_| {
                    metrics::inc_events_error(event.event_type(), "dispatch");
                })?;
                metrics::inc_events_routed(event.event_type(), &decision.agent_type);
            }
            RouteResult::Unroutable { reasoning } => {
                tracing::warn!(
                    event_type = event.event_type(),
                    %reasoning,
                    "event is unroutable"
                );
                if let Err(e) = session
                    .append_routing_decision(
                        event.event_type(),
                        "unroutable",
                        reasoning,
                    )
                    .await
                {
                    warn!(error = %e, "failed to write unroutable entry to transcript");
                }
                metrics::inc_events_unroutable(event.event_type());
                // ── Dead-letter queue ─────────────────────────────────────────
                if let Some(ref prefix) = self.dlq_subject_prefix {
                    let dlq_subject = format!("{prefix}.{}", event.event_type());
                    if let Err(e) = self
                        .nats
                        .publish_with_headers(
                            dlq_subject,
                            HeaderMap::new(),
                            event.payload.clone(),
                        )
                        .await
                    {
                        warn!(error = %e, "failed to publish unroutable event to DLQ");
                    } else {
                        metrics::inc_dlq_published(event.event_type());
                    }
                }
            }
        }

        Ok(result)
    }

    /// Forward the event payload to the concrete actor subject derived from the
    /// routing decision.
    ///
    /// The actor's `nats_subject` pattern uses `>` as the entity wildcard, e.g.
    /// `actors.pr.>`. We replace `>` with the sanitized entity key so the final
    /// subject is something like `actors.pr.owner.repo.456`.
    ///
    /// `agents` must be the same snapshot fetched by `route_event` — passing it
    /// here avoids a second `list_all()` call and eliminates the TOCTOU race
    /// where an agent could unregister between the two lookups.
    async fn dispatch(
        &self,
        event: &RouterEvent,
        decision: &RoutingDecision,
        agents: &[AgentCapability],
    ) -> Result<(), RouterError> {
        let agent = agents
            .iter()
            .find(|a| a.agent_type == decision.agent_type)
            .ok_or_else(|| RouterError::UnknownAgentType(decision.agent_type.clone()))?;

        let sanitized_key = trogon_transcript::subject::sanitize_key(&decision.entity_key);
        let target_subject = agent.nats_subject.replace('>', &sanitized_key);

        // Build headers: carry the original event subject and a timestamp.
        let mut headers = HeaderMap::new();
        headers.insert("Trogon-Original-Subject", event.subject.as_str());
        headers.insert(
            "Trogon-Routed-At",
            now_ms().to_string().as_str(),
        );

        self.nats
            .publish_with_headers(target_subject, headers, event.payload.clone())
            .await
            .map_err(|e| RouterError::Publish(e.to_string()))?;

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_nats::MockNatsClient;
    use trogon_registry::{AgentCapability, MockRegistryStore};
    use trogon_transcript::publisher::mock::MockTranscriptPublisher;

    use crate::{
        decision::{LlmRoutingResponse, RoutingDecision},
        llm::mock::MockLlmClient,
    };

    fn make_router() -> (
        Router<MockLlmClient, MockRegistryStore, MockTranscriptPublisher, MockNatsClient>,
        MockLlmClient,
        MockRegistryStore,
        MockTranscriptPublisher,
        MockNatsClient,
    ) {
        let llm = MockLlmClient::new();
        let store = MockRegistryStore::new();
        let publisher = MockTranscriptPublisher::new();
        let nats = MockNatsClient::new();
        let registry = Registry::new(store.clone());
        let router = Router::new(llm.clone(), registry, publisher.clone(), nats.clone());
        (router, llm, store, publisher, nats)
    }

    fn pr_actor() -> AgentCapability {
        AgentCapability::new("PrActor", ["code_review"], "actors.pr.>")
    }

    #[tokio::test]
    async fn routes_event_to_registered_agent() {
        let (router, llm, store, publisher, nats) = make_router();

        // Register an agent.
        let registry = Registry::new(store.clone());
        registry.register(&pr_actor()).await.unwrap();

        // Queue a routed LLM response.
        llm.push_response(LlmRoutingResponse::Routed {
            agent_type: "PrActor".into(),
            entity_key: "owner/repo/42".into(),
            reasoning: "It's a PR event".into(),
        });

        let event = RouterEvent::new(
            "trogon.events.github.pull_request",
            br#"{"action":"opened","number":42}"#.as_ref(),
        );

        let result = router.route_event(event).await.unwrap();

        assert_eq!(
            result,
            RouteResult::Routed(RoutingDecision {
                agent_type: "PrActor".into(),
                entity_key: "owner/repo/42".into(),
                reasoning: "It's a PR event".into(),
            })
        );

        // Transcript should have one routing decision entry.
        let published = publisher.take_published();
        assert_eq!(published.len(), 1);

        // NATS should have one published message to the actor subject.
        let subjects = nats.published_messages();
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0], "actors.pr.owner.repo.42");
    }

    #[tokio::test]
    async fn unroutable_event_does_not_publish() {
        let (router, llm, store, publisher, nats) = make_router();

        let registry = Registry::new(store.clone());
        registry.register(&pr_actor()).await.unwrap();

        llm.push_response(LlmRoutingResponse::Unroutable {
            reasoning: "no matching agent".into(),
        });

        let event = RouterEvent::new("trogon.events.unknown.thing", b"{}".as_ref());
        let result = router.route_event(event).await.unwrap();

        assert!(matches!(result, RouteResult::Unroutable { .. }));

        // Nothing published to actors.
        let subjects = nats.published_messages();
        assert!(subjects.is_empty());

        // But transcript still gets one unroutable entry.
        let published = publisher.take_published();
        assert_eq!(published.len(), 1);
    }

    #[tokio::test]
    async fn llm_receives_prompt_with_agent_info() {
        let (router, llm, store, _, _nats) = make_router();

        let registry = Registry::new(store.clone());
        registry.register(&pr_actor()).await.unwrap();

        llm.push_response(LlmRoutingResponse::Unroutable {
            reasoning: "test".into(),
        });

        let event = RouterEvent::new(
            "trogon.events.github.pull_request",
            br#"{"action":"closed"}"#.as_ref(),
        );

        router.route_event(event).await.unwrap();

        let prompts = llm.take_prompts();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("PrActor"));
        assert!(prompts[0].contains("github.pull_request"));
    }

    #[tokio::test]
    async fn unknown_agent_type_in_decision_returns_error() {
        let (router, llm, _store, _, _nats) = make_router();
        // Registry is empty — no agents registered.

        llm.push_response(LlmRoutingResponse::Routed {
            agent_type: "GhostActor".into(),
            entity_key: "some/entity".into(),
            reasoning: "hallucinated".into(),
        });

        let event = RouterEvent::new("trogon.events.github.push", b"{}".as_ref());
        let err = router.route_event(event).await.unwrap_err();

        assert!(matches!(err, RouterError::UnknownAgentType(_)));
    }

    /// A publisher that always fails — used to exercise the best-effort
    /// transcript-write warn paths inside `route_event`.
    #[derive(Clone)]
    struct FailPublisher;
    impl trogon_transcript::TranscriptPublisher for FailPublisher {
        async fn publish(
            &self,
            _subject: String,
            _payload: bytes::Bytes,
        ) -> Result<(), trogon_transcript::TranscriptError> {
            Err(trogon_transcript::TranscriptError::Publish(
                "always fails".into(),
            ))
        }
    }

    fn make_router_fail_publisher() -> (
        Router<MockLlmClient, MockRegistryStore, FailPublisher, MockNatsClient>,
        MockLlmClient,
        MockRegistryStore,
        MockNatsClient,
    ) {
        let llm = MockLlmClient::new();
        let store = MockRegistryStore::new();
        let nats = MockNatsClient::new();
        let registry = Registry::new(store.clone());
        let router = Router::new(llm.clone(), registry, FailPublisher, nats.clone());
        (router, llm, store, nats)
    }

    #[tokio::test]
    async fn routed_transcript_write_failure_does_not_fail_route() {
        let (router, llm, store, nats) = make_router_fail_publisher();
        let registry = Registry::new(store.clone());
        registry.register(&pr_actor()).await.unwrap();

        llm.push_response(LlmRoutingResponse::Routed {
            agent_type: "PrActor".into(),
            entity_key: "owner/repo/1".into(),
            reasoning: "test".into(),
        });

        let event = RouterEvent::new("trogon.events.github.push", b"{}".as_ref());
        // Even though transcript write fails, dispatch must still succeed.
        let result = router.route_event(event).await.unwrap();
        assert!(matches!(result, RouteResult::Routed(_)));
        assert_eq!(nats.published_messages().len(), 1);
    }

    #[tokio::test]
    async fn unroutable_transcript_write_failure_does_not_fail_route() {
        let (router, llm, store, _nats) = make_router_fail_publisher();
        let registry = Registry::new(store.clone());
        registry.register(&pr_actor()).await.unwrap();

        llm.push_response(LlmRoutingResponse::Unroutable {
            reasoning: "no match".into(),
        });

        let event = RouterEvent::new("trogon.events.github.push", b"{}".as_ref());
        let result = router.route_event(event).await.unwrap();
        assert!(matches!(result, RouteResult::Unroutable { .. }));
    }

    /// Test `run()`: inject one message, verify it gets routed, drop sender to
    /// end the subscription and verify `run()` returns Ok.
    #[tokio::test]
    async fn run_processes_message_then_exits_on_closed_subscription() {
        let (router, llm, store, _publisher, nats) = make_router();

        let registry = Registry::new(store.clone());
        registry.register(&pr_actor()).await.unwrap();

        llm.push_response(LlmRoutingResponse::Routed {
            agent_type: "PrActor".into(),
            entity_key: "owner/repo/1".into(),
            reasoning: "it is a pr".into(),
        });

        // Pre-inject a message stream BEFORE run() calls subscribe().
        let sender = nats.inject_messages();
        let msg = async_nats::Message {
            subject: "trogon.events.github.push".into(),
            payload: bytes::Bytes::from_static(b"{}"),
            headers: None,
            status: None,
            description: None,
            length: 2,
            reply: None,
        };
        sender.unbounded_send(msg).unwrap();
        drop(sender); // close channel → subscription stream ends → run() exits

        router.run("trogon.events.>").await.unwrap();

        // The message was dispatched to the actor subject.
        let subjects = nats.published_messages();
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0], "actors.pr.owner.repo.1");
    }

    /// Test that `run()` continues after a `route_event` error (warn path).
    #[tokio::test]
    async fn run_continues_after_route_error() {
        let (router, llm, _store, _publisher, nats) = make_router();
        // No agent registered → routing will error with UnknownAgentType.

        llm.push_response(LlmRoutingResponse::Routed {
            agent_type: "GhostActor".into(),
            entity_key: "x/y".into(),
            reasoning: "hallucinated".into(),
        });

        let sender = nats.inject_messages();
        let msg = async_nats::Message {
            subject: "trogon.events.github.push".into(),
            payload: bytes::Bytes::from_static(b"{}"),
            headers: None,
            status: None,
            description: None,
            length: 2,
            reply: None,
        };
        sender.unbounded_send(msg).unwrap();
        drop(sender);

        // run() must return Ok even though route_event errored for the one message.
        router.run("trogon.events.>").await.unwrap();
    }

    /// Line 63: MockNatsClient.subscribe() returns Err when no stream is injected.
    #[tokio::test]
    async fn run_returns_subscribe_error_when_subscribe_fails() {
        let (router, _llm, _store, _publisher, _nats) = make_router();
        // No inject_messages() call → subscribe() returns MockError → mapped to RouterError::Subscribe
        let err = router.run("trogon.events.>").await.unwrap_err();
        assert!(matches!(err, RouterError::Subscribe(_)));
    }

    /// Line 90: registry.list_all() error is mapped to RouterError::Registry.
    #[tokio::test]
    async fn route_event_returns_registry_error_when_list_all_fails() {
        use trogon_registry::RegistryError;
        use trogon_registry::store::RegistryStore;
        use bytes::Bytes;

        #[derive(Clone)]
        struct AlwaysFailStore;
        impl RegistryStore for AlwaysFailStore {
            async fn put(&self, _: &str, _: Bytes) -> Result<(), RegistryError> {
                Err(RegistryError::Put("fail".into()))
            }
            async fn get(&self, _: &str) -> Result<Option<Bytes>, RegistryError> {
                Err(RegistryError::Get("fail".into()))
            }
            async fn delete(&self, _: &str) -> Result<(), RegistryError> {
                Err(RegistryError::Delete("fail".into()))
            }
            async fn keys(&self) -> Result<Vec<String>, RegistryError> {
                Err(RegistryError::List("injected keys error".into()))
            }
        }

        let llm = crate::llm::mock::MockLlmClient::new();
        let registry = trogon_registry::Registry::new(AlwaysFailStore);
        let publisher = MockTranscriptPublisher::new();
        let nats = MockNatsClient::new();
        let router = Router::new(llm, registry, publisher, nats);

        let event = crate::event::RouterEvent::new("trogon.events.x", b"{}".as_ref());
        let err = router.route_event(event).await.unwrap_err();
        assert!(matches!(err, RouterError::Registry(_)));

        // Cover the otherwise-dead put/get/delete methods on AlwaysFailStore.
        let s = AlwaysFailStore;
        assert!(RegistryStore::put(&s, "k", Bytes::new()).await.is_err());
        assert!(RegistryStore::get(&s, "k").await.is_err());
        assert!(RegistryStore::delete(&s, "k").await.is_err());
    }
}
