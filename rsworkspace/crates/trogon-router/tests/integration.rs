use bytes::Bytes;
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_actor::inbox::provision_actor_inbox;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
use trogon_router::{Router, decision::LlmRoutingResponse, error::RouterError, llm::LlmClient};
use trogon_transcript::{publisher::mock::MockTranscriptPublisher, store::TranscriptStore};

// ── Inline mock LLM (cfg(test) is not set for integration tests) ──────────────

#[derive(Clone, Default)]
struct MockLlmClient {
    responses: Arc<Mutex<Vec<LlmRoutingResponse>>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl MockLlmClient {
    fn new() -> Self {
        Self::default()
    }

    fn push_response(&self, r: LlmRoutingResponse) {
        self.responses.lock().unwrap().push(r);
    }

    fn take_prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().drain(..).collect()
    }
}

impl LlmClient for MockLlmClient {
    async fn complete(&self, prompt: String) -> Result<LlmRoutingResponse, RouterError> {
        self.prompts.lock().unwrap().push(prompt);
        let r = self.responses.lock().unwrap().remove(0);
        Ok(r)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn setup() -> (
    async_nats::Client,
    async_nats::jetstream::Context,
    NatsJetStreamClient,
    impl Drop,
) {
    let container = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("failed to start NATS");
    let port = container
        .get_host_port_ipv4(4222)
        .await
        .expect("failed to get port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = async_nats::jetstream::new(nats.clone());
    let js_client = NatsJetStreamClient::new(js.clone());
    provision_actor_inbox(&js_client)
        .await
        .expect("failed to provision ACTOR_INBOX stream");
    (nats, js, js_client, container)
}

fn pr_actor() -> AgentCapability {
    AgentCapability::new(
        "PrActor",
        ["code_review", "security_analysis"],
        "actors.pr.>",
    )
}

/// Publish a message after a brief delay so the router has time to subscribe.
async fn publish_after_subscribe(nats: &async_nats::Client, subject: &str, payload: Bytes) {
    tokio::time::sleep(Duration::from_millis(100)).await;
    nats.publish(subject.to_string(), payload).await.unwrap();
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn router_forwards_event_to_actor_subject() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let mut actor_sub = nats.subscribe("actors.pr.>").await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "owner/repo/42".into(),
        reasoning: "PR opened event".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher, nats.clone(), js_client);

    // Spawn router first, then publish after it has subscribed.
    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.pull_request",
        Bytes::from_static(br#"{"action":"opened","number":42}"#),
    )
    .await;

    let msg = tokio::time::timeout(Duration::from_secs(3), actor_sub.next())
        .await
        .expect("timed out waiting for actor message")
        .expect("subscription ended unexpectedly");

    assert_eq!(msg.subject.as_str(), "actors.pr.owner.repo.42");

    router_handle.abort();
}

#[tokio::test]
async fn router_records_routing_decision_in_transcript() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let _actor_sub = nats.subscribe("actors.pr.>").await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "owner/repo/99".into(),
        reasoning: "This is a PR".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher.clone(), nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.pull_request",
        Bytes::from_static(br#"{"action":"opened","number":99}"#),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    router_handle.abort();

    let published = publisher.take_published();
    assert_eq!(published.len(), 1, "expected one transcript entry");
}

#[tokio::test]
async fn unroutable_event_does_not_forward_to_any_actor() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let mut actors_sub = nats.subscribe("actors.>").await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Unroutable {
        reasoning: "No matching agent for this event type".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher.clone(), nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.unknown.thing",
        Bytes::from_static(b"{}"),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    router_handle.abort();

    // No messages should have been forwarded to any actor subject.
    let received = tokio::time::timeout(Duration::from_millis(50), actors_sub.next()).await;
    assert!(
        received.is_err(),
        "unroutable event should not be forwarded to any actor"
    );
}

#[tokio::test]
async fn router_records_unroutable_entry_in_transcript() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Unroutable {
        reasoning: "registry is empty".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher.clone(), nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.push",
        Bytes::from_static(b"{}"),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    router_handle.abort();

    let published = publisher.take_published();
    assert_eq!(
        published.len(),
        1,
        "should have one transcript entry for the unroutable event"
    );
}

#[tokio::test]
async fn router_uses_real_jetstream_transcript() {
    let (nats, js, js_client, _container) = setup().await;

    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let mut pr_sub = nats.subscribe("actors.pr.>").await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "myorg/myrepo/7".into(),
        reasoning: "PR event".into(),
    });

    use trogon_transcript::publisher::NatsTranscriptPublisher;
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let router = Router::new(llm, registry, publisher, nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.pull_request",
        Bytes::from_static(br#"{"action":"opened","number":7}"#),
    )
    .await;

    let msg = tokio::time::timeout(Duration::from_secs(3), pr_sub.next())
        .await
        .expect("timed out")
        .expect("subscription ended");

    assert_eq!(msg.subject.as_str(), "actors.pr.myorg.myrepo.7");
    router_handle.abort();

    let entries = transcript_store
        .query("router", "trogon.events.github.pull_request")
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        &entries[0],
        trogon_transcript::TranscriptEntry::RoutingDecision { to, .. } if to == "PrActor"
    ));
}

/// The router attaches two metadata headers to every forwarded message.
/// This test subscribes to the actor subject and checks both headers are present
/// with the expected values.
#[tokio::test]
async fn forwarded_message_carries_routing_headers() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let mut actor_sub = nats.subscribe("actors.pr.>").await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "owner/repo/1".into(),
        reasoning: "PR event".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher, nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.pull_request",
        Bytes::from_static(br#"{"action":"opened","number":1}"#),
    )
    .await;

    let msg = tokio::time::timeout(Duration::from_secs(3), actor_sub.next())
        .await
        .expect("timed out")
        .expect("subscription ended");

    router_handle.abort();

    let headers = msg.headers.expect("message should have headers");
    assert_eq!(
        headers.get("Trogon-Original-Subject").map(|v| v.as_str()),
        Some("trogon.events.github.pull_request"),
        "Trogon-Original-Subject header should carry the source subject"
    );
    assert!(
        headers.get("Trogon-Routed-At").is_some(),
        "Trogon-Routed-At header should be present"
    );
    // Routed-At must be a valid millisecond timestamp (large positive integer).
    let routed_at: u64 = headers
        .get("Trogon-Routed-At")
        .unwrap()
        .as_str()
        .parse()
        .expect("Trogon-Routed-At should be a numeric timestamp");
    assert!(routed_at > 0, "timestamp should be non-zero");
}

/// Send two events: the first causes an `UnknownAgentType` error (LLM returns
/// an agent type not in the registry). The second is correctly routed.
/// Verifies that the router loop's `warn! and continue` behaviour means the
/// second event is still processed even after the first fails.
#[tokio::test]
async fn router_loop_continues_after_routing_error() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let mut actor_sub = nats.subscribe("actors.pr.>").await.unwrap();

    let llm = MockLlmClient::new();
    // First event: LLM returns a ghost agent type not in the registry.
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "GhostActor".into(),
        entity_key: "some/key/1".into(),
        reasoning: "hallucinated".into(),
    });
    // Second event: LLM returns a valid routing decision.
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "owner/repo/99".into(),
        reasoning: "valid PR event".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher, nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    // Publish the first event (triggers UnknownAgentType error).
    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.push",
        Bytes::from_static(b"{}"),
    )
    .await;

    // Allow the router time to process the first (failed) event.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Publish the second event — the router must still be running and route it.
    nats_clone
        .publish(
            "trogon.events.github.pull_request",
            Bytes::from_static(br#"{"number":99}"#),
        )
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(3), actor_sub.next())
        .await
        .expect("timed out — router may have stopped after the first error")
        .expect("subscription ended unexpectedly");

    assert_eq!(
        msg.subject.as_str(),
        "actors.pr.owner.repo.99",
        "second event should be forwarded after first event error"
    );

    router_handle.abort();
}

#[tokio::test]
async fn router_prompt_contains_live_registry_data() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Unroutable {
        reasoning: "test only".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm.clone(), registry, publisher, nats.clone(), js_client);

    let nats_clone = nats.clone();
    let router_handle = tokio::spawn(async move {
        router.run(&["trogon.events.>"]).await.ok();
    });

    publish_after_subscribe(
        &nats_clone,
        "trogon.events.github.pull_request",
        Bytes::from_static(br#"{"action":"closed"}"#),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    router_handle.abort();

    let prompts = llm.take_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("PrActor"),
        "prompt should include registered agent"
    );
    assert!(
        prompts[0].contains("code_review"),
        "prompt should include agent capabilities"
    );
    assert!(
        prompts[0].contains("github.pull_request"),
        "prompt should include event type"
    );
}

/// When the router is started with multiple subjects, events published on any
/// of them are received and routed — each subject is subscribed independently.
#[tokio::test]
async fn router_receives_events_from_multiple_subjects() {
    let (nats, js, js_client, _container) = setup().await;

    let store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(store);
    registry.register(&pr_actor()).await.unwrap();

    let mut actor_sub = nats.subscribe("actors.pr.>").await.unwrap();

    let llm = MockLlmClient::new();
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "org/repo/1".into(),
        reasoning: "github event".into(),
    });
    llm.push_response(LlmRoutingResponse::Routed {
        agent_type: "PrActor".into(),
        entity_key: "org/repo/2".into(),
        reasoning: "linear event".into(),
    });

    let publisher = MockTranscriptPublisher::new();
    let router = Router::new(llm, registry, publisher, nats.clone(), js_client);

    let router_handle = tokio::spawn(async move {
        router.run(&["github.>", "linear.>"]).await.ok();
    });

    // Let router subscribe before publishing.
    tokio::time::sleep(Duration::from_millis(100)).await;
    nats.publish(
        "github.pull_request",
        Bytes::from_static(br#"{"action":"opened","number":1}"#),
    )
    .await
    .unwrap();
    nats.publish(
        "linear.issue",
        Bytes::from_static(br#"{"type":"Issue","action":"create"}"#),
    )
    .await
    .unwrap();

    let msg1 = tokio::time::timeout(Duration::from_secs(3), actor_sub.next())
        .await
        .expect("timed out waiting for first actor message")
        .expect("subscription ended");
    let msg2 = tokio::time::timeout(Duration::from_secs(3), actor_sub.next())
        .await
        .expect("timed out waiting for second actor message")
        .expect("subscription ended");

    assert!(
        msg1.subject.as_str().starts_with("actors.pr."),
        "first event must be routed to actor"
    );
    assert!(
        msg2.subject.as_str().starts_with("actors.pr."),
        "second event must be routed to actor"
    );
    assert_ne!(
        msg1.subject, msg2.subject,
        "events from different subjects must produce distinct routing keys"
    );

    router_handle.abort();
}
