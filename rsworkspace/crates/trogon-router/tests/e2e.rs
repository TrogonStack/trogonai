/// End-to-end test: real NATS, Router + ActorHost wired together.
///
/// Verifies the full path: event published → router routes via LLM → actor
/// subject → ActorHost dispatches → actor handles → state persisted → both
/// routing-decision and actor-message entries land in the transcript.
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use trogon_actor::{
    ActorContext, ActorRuntime, EntityActor, StateStore, host::ActorHost, provision_state,
};
use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
use trogon_router::{
    Router,
    decision::LlmRoutingResponse,
    error::RouterError,
    llm::LlmClient,
};
use trogon_transcript::{
    entry::TranscriptEntry,
    publisher::NatsTranscriptPublisher,
    store::TranscriptStore,
};

// ── Test actor ────────────────────────────────────────────────────────────────

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ScribeState {
    count: u32,
}

#[derive(Clone)]
struct ScribeActor;

impl EntityActor for ScribeActor {
    type State = ScribeState;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "scribe"
    }

    async fn handle(
        &mut self,
        state: &mut ScribeState,
        ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.count += 1;
        ctx.append_user_message(
            &format!("event received (count={})", state.count),
            None,
        )
        .await
        .ok();
        ctx.append_assistant_message("acknowledged", None).await.ok();
        Ok(())
    }
}

// ── Mock LLM ──────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct MockLlmClient {
    responses: Arc<Mutex<Vec<LlmRoutingResponse>>>,
}

impl MockLlmClient {
    fn with_response(r: LlmRoutingResponse) -> Self {
        Self { responses: Arc::new(Mutex::new(vec![r])) }
    }
}

impl LlmClient for MockLlmClient {
    async fn complete(&self, _prompt: String) -> Result<LlmRoutingResponse, RouterError> {
        Ok(self.responses.lock().unwrap().remove(0))
    }
}

// ── Setup ─────────────────────────────────────────────────────────────────────

async fn setup() -> (async_nats::Client, async_nats::jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("failed to start NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = async_nats::jetstream::new(nats.clone());
    (nats, js, container)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Full path: trogon.events.* → Router → actors.scribe.* → ScribeActor.
/// After processing, both the routing decision and the actor transcript entries
/// must be present in the TRANSCRIPTS JetStream stream.
#[tokio::test]
async fn router_routes_to_actor_and_both_write_transcript() {
    let (nats, js, _container) = setup().await;

    // ── Provision shared infrastructure ──────────────────────────────────────
    let state_store = provision_state(&js).await.unwrap();
    let reg_store = provision_registry(&js).await.unwrap();
    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());

    // ── Register the scribe capability so the router can discover it ──────────
    let scribe_cap =
        AgentCapability::new("ScribeActor", ["scribing"], "actors.scribe.>");
    let registry_for_router = Registry::new(reg_store.clone());
    registry_for_router.register(&scribe_cap).await.unwrap();

    // ── Build and start ActorHost ─────────────────────────────────────────────
    let registry_for_actor = Registry::new(reg_store.clone());
    let actor_runtime = ActorRuntime::new(
        state_store,
        publisher.clone(),
        nats.clone(),
        registry_for_actor,
    );
    let host = ActorHost::new(actor_runtime, ScribeActor, scribe_cap.clone());
    let host_handle = tokio::spawn(async move {
        host.run().await.ok();
    });

    // ── Build and start Router ────────────────────────────────────────────────
    let llm = MockLlmClient::with_response(LlmRoutingResponse::Routed {
        agent_type: "ScribeActor".into(),
        // entity_key uses "/" separators — router sanitizes to dots
        entity_key: "test/entity/1".into(),
        reasoning: "this is a scribing event".into(),
    });
    let router = Router::new(llm, registry_for_router, publisher, nats.clone());
    let router_handle = tokio::spawn(async move {
        router.run("trogon.events.>").await.ok();
    });

    // ── Let both services subscribe before publishing ─────────────────────────
    tokio::time::sleep(Duration::from_millis(200)).await;

    nats.publish(
        "trogon.events.github.pull_request",
        Bytes::from_static(br#"{"action":"opened","number":1}"#),
    )
    .await
    .unwrap();

    // ── Wait for processing ───────────────────────────────────────────────────
    // Poll until the actor transcript appears (up to 3 s).
    let actor_entries = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let entries = transcript_store
                .query("scribe", "test.entity.1")
                .await
                .unwrap();
            if entries.len() >= 2 {
                return entries;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for actor transcript entries");

    host_handle.abort();
    router_handle.abort();

    // ── Assert routing-decision transcript ────────────────────────────────────
    let router_entries = transcript_store
        .query("router", "trogon.events.github.pull_request")
        .await
        .unwrap();

    assert_eq!(
        router_entries.len(),
        1,
        "router should write exactly one RoutingDecision entry"
    );
    assert!(
        matches!(&router_entries[0], TranscriptEntry::RoutingDecision { to, .. } if to == "ScribeActor"),
        "routing decision should target ScribeActor, got: {:?}",
        router_entries[0]
    );

    // ── Assert actor transcript ───────────────────────────────────────────────
    assert_eq!(
        actor_entries.len(),
        2,
        "actor should write user + assistant transcript entries"
    );
    assert!(
        matches!(&actor_entries[0], TranscriptEntry::Message { role: trogon_transcript::entry::Role::User, .. }),
        "first actor entry should be a User message"
    );
    assert!(
        matches!(&actor_entries[1], TranscriptEntry::Message { role: trogon_transcript::entry::Role::Assistant, .. }),
        "second actor entry should be an Assistant message"
    );
}

/// The actor's state must be persisted in the ACTOR_STATE KV bucket after
/// the event is processed.
#[tokio::test]
async fn actor_state_is_persisted_after_routing() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let reg_store = provision_registry(&js).await.unwrap();
    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());

    let scribe_cap =
        AgentCapability::new("ScribeActor", ["scribing"], "actors.scribe.>");
    let registry_for_router = Registry::new(reg_store.clone());
    registry_for_router.register(&scribe_cap).await.unwrap();

    let registry_for_actor = Registry::new(reg_store.clone());
    let actor_runtime = ActorRuntime::new(
        state_store.clone(),
        publisher.clone(),
        nats.clone(),
        registry_for_actor,
    );
    let host = ActorHost::new(actor_runtime, ScribeActor, scribe_cap.clone());
    let host_handle = tokio::spawn(async move {
        host.run().await.ok();
    });

    let llm = MockLlmClient::with_response(LlmRoutingResponse::Routed {
        agent_type: "ScribeActor".into(),
        entity_key: "acme/app/7".into(),
        reasoning: "test state persistence".into(),
    });
    let router = Router::new(llm, registry_for_router, publisher, nats.clone());
    let router_handle = tokio::spawn(async move {
        router.run("trogon.events.>").await.ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    nats.publish("trogon.events.github.push", Bytes::from_static(b"{}"))
        .await
        .unwrap();

    // Poll KV until state appears (up to 3 s).
    let kv_key = trogon_actor::state_kv_key("scribe", "acme.app.7");
    let raw_state: Bytes = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(Some(entry)) = state_store.load(&kv_key).await {
                return entry.value;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for actor state in KV");

    host_handle.abort();
    router_handle.abort();

    let state: ScribeState = serde_json::from_slice(&raw_state).unwrap();
    assert_eq!(state.count, 1, "actor should have processed exactly one event");
}
