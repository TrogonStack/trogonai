use bytes::Bytes;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use trogon_actor::{
    context::ActorContext,
    actor::EntityActor,
    error::ActorError,
    runtime::ActorRuntime,
    state::provision_state,
};
use trogon_registry::{Registry, provision as provision_registry};
use trogon_transcript::{
    publisher::NatsTranscriptPublisher,
    store::TranscriptStore,
    entry::TranscriptEntry,
};

async fn setup() -> (async_nats::Client, async_nats::jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("failed to start NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("failed to get port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = async_nats::jetstream::new(nats.clone());
    (nats, js, container)
}

// ── Test actor ────────────────────────────────────────────────────────────────

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Counter {
    count: u32,
}

struct CounterActor;

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
        Ok(())
    }
}

// ── Actor that records a transcript entry ─────────────────────────────────────

struct Scribe;

impl EntityActor for Scribe {
    type State = Counter;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "scribe"
    }

    async fn handle(
        &mut self,
        state: &mut Counter,
        ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.count += 1;
        ctx.append_user_message("user prompt", None).await.ok();
        ctx.append_assistant_message("assistant reply", Some(5)).await.ok();
        Ok(())
    }
}

// ── Actor that calls on_create ────────────────────────────────────────────────

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct InitState {
    initialized: bool,
    count: u32,
}

struct InitActor;

impl EntityActor for InitActor {
    type State = InitState;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "init-actor"
    }

    async fn on_create(state: &mut InitState) -> Result<(), Self::Error> {
        state.initialized = true;
        Ok(())
    }

    async fn handle(
        &mut self,
        state: &mut InitState,
        _ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.count += 1;
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn provision_state_creates_actor_state_bucket() {
    let (_nats, js, _container) = setup().await;

    provision_state(&js).await.expect("provision_state failed");

    js.get_key_value("ACTOR_STATE")
        .await
        .expect("ACTOR_STATE bucket not found after provision");
}

#[tokio::test]
async fn provision_state_is_idempotent() {
    let (_nats, js, _container) = setup().await;

    provision_state(&js).await.expect("first provision failed");
    provision_state(&js).await.expect("second provision should not fail");
}

#[tokio::test]
async fn state_persists_across_events() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry);

    runtime.handle_event(&mut CounterActor, "entity-1").await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-1").await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-1").await.unwrap();

    // Load state directly to verify persistence.
    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let entry = kv.entry("counter.entity-1").await.unwrap().unwrap();
    let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
    assert_eq!(saved.count, 3);
}

#[tokio::test]
async fn on_create_called_on_first_event_only() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry);

    // First event: on_create sets initialized = true.
    runtime.handle_event(&mut InitActor, "e-1").await.unwrap();

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("init-actor.e-1").await.unwrap().unwrap().value;
    let state: InitState = serde_json::from_slice(&bytes).unwrap();
    assert!(state.initialized);
    assert_eq!(state.count, 1);

    // Second event: on_create is NOT called again.
    runtime.handle_event(&mut InitActor, "e-1").await.unwrap();
    let bytes2 = kv.entry("init-actor.e-1").await.unwrap().unwrap().value;
    let state2: InitState = serde_json::from_slice(&bytes2).unwrap();
    assert!(state2.initialized);
    assert_eq!(state2.count, 2);
}

#[tokio::test]
async fn different_entities_have_independent_state() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry);

    runtime.handle_event(&mut CounterActor, "entity-A").await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-A").await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-B").await.unwrap();

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();

    let a_bytes = kv.entry("counter.entity-A").await.unwrap().unwrap().value;
    let a: Counter = serde_json::from_slice(&a_bytes).unwrap();
    assert_eq!(a.count, 2);

    let b_bytes = kv.entry("counter.entity-B").await.unwrap().unwrap().value;
    let b: Counter = serde_json::from_slice(&b_bytes).unwrap();
    assert_eq!(b.count, 1);
}

// ── New tests ─────────────────────────────────────────────────────────────────

/// Two concurrent handle_event calls to the same entity key exercise the real
/// OCC path in NATS KV. Neither call should fail and the final count must
/// equal 2 — no lost writes regardless of how NATS schedules the two saves.
#[tokio::test]
async fn concurrent_events_to_same_entity_no_lost_writes() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry);
    let runtime2 = runtime.clone();

    // Fire two concurrent handle_event calls at the same entity.
    let mut actor1 = CounterActor;
    let mut actor2 = CounterActor;
    let (r1, r2) = tokio::join!(
        runtime.handle_event(&mut actor1, "entity-concurrent"),
        runtime2.handle_event(&mut actor2, "entity-concurrent"),
    );

    assert!(r1.is_ok(), "first concurrent handler failed: {r1:?}");
    assert!(r2.is_ok(), "second concurrent handler failed: {r2:?}");

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("counter.entity-concurrent").await.unwrap().unwrap().value;
    let saved: Counter = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved.count, 2, "both events must be reflected in final state");
}

/// When the KV store holds a value that cannot be deserialized into the actor's
/// State type, handle_event must return ActorError::Deserialize — not panic.
#[tokio::test]
async fn corrupted_state_in_kv_returns_deserialize_error() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    // Write corrupt bytes directly into the KV store under the actor's key.
    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    kv.put("counter.entity-corrupt", Bytes::from_static(b"not valid json {{{{"))
        .await
        .unwrap();

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry);
    let err = runtime
        .handle_event(&mut CounterActor, "entity-corrupt")
        .await
        .unwrap_err();

    assert!(
        matches!(err, ActorError::Deserialize(_)),
        "expected Deserialize error, got: {err:?}"
    );
}

#[tokio::test]
async fn transcript_entries_written_to_jetstream() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    // Provision the TRANSCRIPTS stream so the publisher can ack.
    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry);
    runtime.handle_event(&mut Scribe, "entity-1").await.unwrap();

    let entries = transcript_store.query("scribe", "entity-1").await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0], TranscriptEntry::Message { role: trogon_transcript::Role::User, .. }));
    assert!(matches!(&entries[1], TranscriptEntry::Message { role: trogon_transcript::Role::Assistant, tokens: Some(5), .. }));
}
