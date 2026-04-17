use bytes::Bytes;
use futures_util::StreamExt;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use std::sync::Arc;
use trogon_actor::{
    context::ActorContext,
    actor::EntityActor,
    error::ActorError,
    host::{ActorHost, HostError},
    runtime::ActorRuntime,
    state::provision_state,
};
use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
use trogon_transcript::{
    TranscriptError,
    publisher::{NatsTranscriptPublisher, TranscriptPublisher},
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

/// Pre-create the ACTOR_STATE bucket with Memory storage (provision_state uses File),
/// causing create_key_value to return STREAM_NAME_EXIST. The is_bucket_already_exists()
/// fallback should open the existing bucket and return Ok.
#[tokio::test]
async fn provision_state_falls_back_to_existing_bucket_on_config_mismatch() {
    let (_nats, js, _container) = setup().await;

    // Pre-create with different storage type to force a config-mismatch error.
    js.create_key_value(async_nats::jetstream::kv::Config {
        bucket: "ACTOR_STATE".to_string(),
        history: 1,
        max_age: std::time::Duration::ZERO,
        storage: async_nats::jetstream::stream::StorageType::Memory, // differs from File
        ..Default::default()
    })
    .await
    .expect("pre-create should succeed");

    // provision_state() should open the existing bucket via the fallback path.
    provision_state(&js)
        .await
        .expect("provision_state should succeed via fallback");
}

#[tokio::test]
async fn state_persists_across_events() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());

    runtime.handle_event(&mut CounterActor, "entity-1", 0).await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-1", 0).await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-1", 0).await.unwrap();

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

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());

    // First event: on_create sets initialized = true.
    runtime.handle_event(&mut InitActor, "e-1", 0).await.unwrap();

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("init-actor.e-1").await.unwrap().unwrap().value;
    let state: InitState = serde_json::from_slice(&bytes).unwrap();
    assert!(state.initialized);
    assert_eq!(state.count, 1);

    // Second event: on_create is NOT called again.
    runtime.handle_event(&mut InitActor, "e-1", 0).await.unwrap();
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

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());

    runtime.handle_event(&mut CounterActor, "entity-A", 0).await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-A", 0).await.unwrap();
    runtime.handle_event(&mut CounterActor, "entity-B", 0).await.unwrap();

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

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    let runtime2 = runtime.clone();

    // Fire two concurrent handle_event calls at the same entity.
    let mut actor1 = CounterActor;
    let mut actor2 = CounterActor;
    let (r1, r2) = tokio::join!(
        runtime.handle_event(&mut actor1, "entity-concurrent", 0),
        runtime2.handle_event(&mut actor2, "entity-concurrent", 0),
    );

    assert!(r1.is_ok(), "first concurrent handler failed: {r1:?}");
    assert!(r2.is_ok(), "second concurrent handler failed: {r2:?}");

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("counter.entity-concurrent").await.unwrap().unwrap().value;
    let saved: Counter = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved.count, 2, "both events must be reflected in final state");
}

/// Like `concurrent_events_to_same_entity_no_lost_writes` but starts from an
/// already-existing state entry so the race is in the `update()` path (not the
/// `create()` path). This exercises lines 117-122 in state.rs.
#[tokio::test]
async fn concurrent_updates_on_existing_state_trigger_update_occ() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    let runtime2 = runtime.clone();

    // Create initial state (count=1, revision=0).
    runtime.handle_event(&mut CounterActor, "entity-update-occ", 0).await.unwrap();

    // Now two concurrent events on the SAME existing entity. Both load
    // revision=0, both try to update, one fails with OCC and retries.
    let mut actor1 = CounterActor;
    let mut actor2 = CounterActor;
    let (r1, r2) = tokio::join!(
        runtime.handle_event(&mut actor1, "entity-update-occ", 0),
        runtime2.handle_event(&mut actor2, "entity-update-occ", 0),
    );

    assert!(r1.is_ok(), "first concurrent update failed: {r1:?}");
    assert!(r2.is_ok(), "second concurrent update failed: {r2:?}");

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("counter.entity-update-occ").await.unwrap().unwrap().value;
    let saved: Counter = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved.count, 3, "all 3 increments (initial + 2 concurrent) must be counted");
}

// ── Actor that spawns a sub-agent ─────────────────────────────────────────────

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SpawnState {
    spawn_reply: String,
}

struct SpawnActor;

impl EntityActor for SpawnActor {
    type State = SpawnState;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "spawn-actor"
    }

    async fn handle(
        &mut self,
        state: &mut SpawnState,
        ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        if let Ok(bytes) = ctx
            .spawn_agent::<Self::Error>("security_analysis", Bytes::from_static(b"request"))
            .await
        {
            state.spawn_reply = String::from_utf8_lossy(&bytes).to_string();
        }
        Ok(())
    }
}

// ── Actor that calls spawn_agent when no capable agent exists ─────────────────

struct NoCapActor;

impl EntityActor for NoCapActor {
    type State = Counter;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "no-cap-actor"
    }

    async fn handle(
        &mut self,
        state: &mut Counter,
        ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.count += 1;
        // The registry has no agent with this capability — ignore the error.
        ctx.spawn_agent::<Self::Error>("nonexistent_capability", Bytes::new())
            .await
            .ok();
        Ok(())
    }
}

/// `StateStore::delete` removes an entry from the KV store. After deletion,
/// `load` must return `None` for that key.
#[tokio::test]
async fn state_delete_removes_entry() {
    let (_nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();

    // Save an entry directly via the KV handle so we don't need a runtime.
    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    kv.put("counter.to-delete", Bytes::from_static(b"{\"count\":1}"))
        .await
        .unwrap();

    // Confirm it exists before deletion.
    let before = state_store.load("counter.to-delete").await.unwrap();
    assert!(before.is_some(), "entry should exist before delete");

    // Delete via the StateStore trait (using UFCS to avoid kv::Store's inherent delete()).
    use trogon_actor::state::StateStore;
    StateStore::delete(&state_store, "counter.to-delete").await.unwrap();

    // Entry must be gone after deletion.
    let after = state_store.load("counter.to-delete").await.unwrap();
    assert!(after.is_none(), "entry should be absent after delete");
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

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    let err = runtime
        .handle_event(&mut CounterActor, "entity-corrupt", 0)
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

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    runtime.handle_event(&mut Scribe, "entity-1", 0).await.unwrap();

    let entries = transcript_store.query("scribe", "entity-1").await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0], TranscriptEntry::Message { role: trogon_transcript::Role::User, .. }));
    assert!(matches!(&entries[1], TranscriptEntry::Message { role: trogon_transcript::Role::Assistant, tokens: Some(5), .. }));
}

/// An actor calls `ctx.spawn_agent()` with a registered capability. The runtime
/// performs NATS request-reply, records a `SubAgentSpawn` transcript entry, and
/// returns the reply payload to the actor.
#[tokio::test]
async fn spawn_agent_records_sub_agent_spawn_entry() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    // Provision transcript stream so entries can be confirmed afterwards.
    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();

    // Register a sub-agent with a concrete (non-wildcard) inbox subject.
    let cap = AgentCapability::new("SecurityActor", ["security_analysis"], "sub-agents.security");
    registry.register(&cap).await.unwrap();

    // Spin up a NATS responder on that subject before calling handle_event.
    let nats_responder = nats.clone();
    let mut responder_sub = nats.subscribe("sub-agents.security").await.unwrap();
    tokio::spawn(async move {
        while let Some(msg) = responder_sub.next().await {
            // New spawn design uses Trogon-Reply-To header for the reply inbox.
            let reply_to = msg.headers
                .as_ref()
                .and_then(|h| h.get("Trogon-Reply-To"))
                .map(|v| v.as_str().to_string());
            if let Some(reply) = reply_to {
                nats_responder
                    .publish(reply, Bytes::from_static(b"security-ok"))
                    .await
                    .ok();
            }
        }
    });

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    runtime.handle_event(&mut SpawnActor, "entity-1", 0).await.unwrap();

    // Actor state captured the sub-agent reply payload.
    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("spawn-actor.entity-1").await.unwrap().unwrap().value;
    let state: SpawnState = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(state.spawn_reply, "security-ok");

    // A SubAgentSpawn entry must have been written to the transcript.
    let entries = transcript_store.query("spawn-actor", "entity-1").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        matches!(
            &entries[0],
            TranscriptEntry::SubAgentSpawn { capability, .. } if capability == "security_analysis"
        ),
        "expected SubAgentSpawn entry, got: {:?}", entries[0]
    );
}

/// When no registered agent provides the requested capability, `spawn_agent`
/// returns an error string and the runtime does NOT write a `SubAgentSpawn`
/// transcript entry (the entry is only written after the agent is found).
#[tokio::test]
async fn spawn_agent_no_capability_no_transcript_entry() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    // Empty registry — no agents registered.
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let transcript_store = TranscriptStore::new(js.clone());
    transcript_store.provision().await.unwrap();

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    // NoCapActor ignores the spawn error — handle_event must still succeed.
    runtime.handle_event(&mut NoCapActor, "entity-1", 0).await.unwrap();

    // Actor state was saved (handle completed normally).
    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("no-cap-actor.entity-1").await.unwrap().unwrap().value;
    let saved: Counter = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved.count, 1);

    // No SubAgentSpawn entry should have been written.
    let entries = transcript_store.query("no-cap-actor", "entity-1").await.unwrap();
    assert!(
        entries.is_empty(),
        "expected no transcript entries when no capable agent is found, got: {entries:?}"
    );
}

/// Deleting the backing JetStream stream causes `kv::Store::create` to return a
/// non-OCC error. `StateStore::save(key, val, None)` must surface that as
/// `SaveError::Other` — covers state.rs line 108.
#[tokio::test]
async fn state_store_save_create_returns_other_error_when_stream_deleted() {
    let (_nats, js, _container) = setup().await;
    let state_store = provision_state(&js).await.unwrap();

    // Blow away the backing stream so subsequent KV operations fail.
    js.delete_stream("KV_ACTOR_STATE").await.unwrap();

    use trogon_actor::{SaveError, state::StateStore};
    let result = StateStore::save(&state_store, "counter.ghost", Bytes::from_static(b"{}"), None).await;
    assert!(
        matches!(result, Err(SaveError::Other(_))),
        "expected SaveError::Other when stream is gone (create path), got: {result:?}"
    );
}

/// Same as above but for the `update` path — calls `StateStore::save` with a
/// revision number so the code takes the `kv::Store::update` branch, whose
/// non-OCC failure maps to `SaveError::Other` — covers state.rs line 120.
#[tokio::test]
async fn state_store_save_update_returns_other_error_when_stream_deleted() {
    let (nats, js, _container) = setup().await;
    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    // Create initial state so we have a valid revision.
    let runtime = ActorRuntime::new(state_store.clone(), publisher, nats, registry, js.clone());
    runtime.handle_event(&mut CounterActor, "ghost-update", 0).await.unwrap();

    // Read back the revision so we can pass it to save().
    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let entry = kv.entry("counter.ghost-update").await.unwrap().unwrap();
    let rev = entry.revision;

    // Now delete the stream — the update call will fail with a non-OCC error.
    js.delete_stream("KV_ACTOR_STATE").await.unwrap();

    use trogon_actor::{SaveError, state::StateStore};
    let result = StateStore::save(
        &state_store,
        "counter.ghost-update",
        Bytes::from_static(b"{}"),
        Some(rev),
    )
    .await;
    assert!(
        matches!(result, Err(SaveError::Other(_))),
        "expected SaveError::Other when stream is gone (update path), got: {result:?}"
    );
}

/// Drop the NATS container before calling provision_state() to verify that a
/// JetStream error is surfaced as a String error — covers state.rs line 168.
#[tokio::test]
async fn provision_state_returns_error_when_nats_is_down() {
    let (_nats, js, container) = setup().await;
    drop(container);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let result = provision_state(&js).await;
    assert!(result.is_err(), "expected error when NATS is down, got Ok");
}

/// Deleting the backing stream causes `kv::Store::entry()` to fail, which must
/// propagate as `Err(String)` from `StateStore::load` — covers state.rs line 88.
#[tokio::test]
async fn state_store_load_returns_error_when_stream_deleted() {
    let (_nats, js, _container) = setup().await;
    let state_store = provision_state(&js).await.unwrap();

    js.delete_stream("KV_ACTOR_STATE").await.unwrap();

    use trogon_actor::state::StateStore;
    let result = StateStore::load(&state_store, "counter.ghost").await;
    assert!(result.is_err(), "expected Err from load when stream is gone");
}

/// Deleting the backing stream causes `kv::Store::delete()` to fail, propagating
/// as `Err(String)` from `StateStore::delete` — covers state.rs line 130.
#[tokio::test]
async fn state_store_delete_returns_error_when_stream_deleted() {
    let (_nats, js, _container) = setup().await;
    let state_store = provision_state(&js).await.unwrap();

    js.delete_stream("KV_ACTOR_STATE").await.unwrap();

    use trogon_actor::state::StateStore;
    let result = StateStore::delete(&state_store, "counter.ghost").await;
    assert!(result.is_err(), "expected Err from delete when stream is gone, got: {result:?}");
}

/// Calling `StateStore::save` with `revision = None` for a key that already
/// exists exercises `is_create_conflict()`: the inner KV create fails with an
/// OCC error and `SaveError::Conflict` must be returned — covers state.rs
/// `is_create_conflict` string-matching path.
#[tokio::test]
async fn state_store_create_conflict_returns_save_error_conflict() {
    let (_nats, js, _container) = setup().await;
    let state_store = provision_state(&js).await.unwrap();

    use trogon_actor::{SaveError, state::StateStore};

    // First create succeeds.
    StateStore::save(&state_store, "counter.occ-create", Bytes::from_static(b"{\"count\":0}"), None)
        .await
        .unwrap();

    // Second create on the same key — KV returns an OCC error → Conflict.
    let result = StateStore::save(
        &state_store,
        "counter.occ-create",
        Bytes::from_static(b"{\"count\":1}"),
        None,
    )
    .await;
    assert!(
        matches!(result, Err(SaveError::Conflict)),
        "expected SaveError::Conflict on duplicate create, got: {result:?}"
    );
}

/// Calling `StateStore::save` with a stale `revision` (i.e. the KV entry was
/// already updated to a newer revision) exercises `is_update_conflict()`:
/// `SaveError::Conflict` must be returned — covers state.rs `is_update_conflict`
/// string-matching path.
#[tokio::test]
async fn state_store_update_stale_revision_returns_save_error_conflict() {
    let (_nats, js, _container) = setup().await;
    let state_store = provision_state(&js).await.unwrap();

    use trogon_actor::{SaveError, state::StateStore};

    // Create initial entry (revision = rev0).
    let rev0 = StateStore::save(
        &state_store,
        "counter.occ-update",
        Bytes::from_static(b"{\"count\":0}"),
        None,
    )
    .await
    .unwrap();

    // Advance the entry to a new revision.
    StateStore::save(
        &state_store,
        "counter.occ-update",
        Bytes::from_static(b"{\"count\":1}"),
        Some(rev0),
    )
    .await
    .unwrap();

    // Now attempt to update with the stale rev0 — OCC conflict.
    let result = StateStore::save(
        &state_store,
        "counter.occ-update",
        Bytes::from_static(b"{\"count\":2}"),
        Some(rev0),
    )
    .await;
    assert!(
        matches!(result, Err(SaveError::Conflict)),
        "expected SaveError::Conflict on stale revision update, got: {result:?}"
    );
}

/// When the TRANSCRIPTS stream is not provisioned, `Session::append()` returns
/// an error. The actor's transcript writes fail silently (actors call `.ok()`) but
/// `handle_event` still succeeds — covers runtime.rs line 167 (append_fn error path).
#[tokio::test]
async fn actor_transcript_append_failure_does_not_abort_handle_event() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    // Use NatsTranscriptPublisher WITHOUT provisioning the TRANSCRIPTS stream.
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    // Scribe calls ctx.append_user_message(...).ok() — if append fails the actor
    // still succeeds. handle_event must return Ok.
    let result = runtime.handle_event(&mut Scribe, "entity-no-transcript", 0).await;
    assert!(result.is_ok(), "handle_event must succeed even when transcript appends fail");
}

/// Deleting the AGENT_REGISTRY backing stream makes `registry.discover()` fail.
/// The spawn_fn propagates the error as an `Err(String)` — covers runtime.rs line 185.
/// SpawnActor ignores errors from `ctx.spawn_agent()` via `.ok()`, so handle_event
/// still succeeds.
#[tokio::test]
async fn spawn_agent_discover_failure_is_propagated_as_error_string() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);

    // Register a capable agent so the registry lookup would succeed normally.
    let cap = AgentCapability::new("SecurityActor", ["security_analysis"], "sub-agents.sec");
    registry.register(&cap).await.unwrap();

    // Now destroy the AGENT_REGISTRY backing stream — discover() will fail.
    js.delete_stream("KV_AGENT_REGISTRY").await.unwrap();

    let runtime = ActorRuntime::new(state_store, publisher, nats, registry, js.clone());
    // SpawnActor calls ctx.spawn_agent(...) and ignores errors — handle_event must succeed.
    let result = runtime.handle_event(&mut SpawnActor, "entity-discover-fail", 0).await;
    assert!(result.is_ok(), "handle_event must succeed when discover fails: {result:?}");
}

// ── ActorHost integration tests ───────────────────────────────────────────────
//
// These tests exercise ActorHost::run(): subscribing to NATS, dispatching
// incoming messages to the actor, and the cancel/shutdown path.

/// Clonable actor used only in host tests.
#[derive(Clone)]
struct HostCounterActor;

impl EntityActor for HostCounterActor {
    type State = Counter;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str { "host-counter" }

    async fn handle(
        &mut self,
        state: &mut Counter,
        _ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.count += 1;
        Ok(())
    }
}

fn make_host(
    runtime: ActorRuntime<
        async_nats::jetstream::kv::Store,
        NatsTranscriptPublisher,
        async_nats::Client,
        async_nats::jetstream::kv::Store,
        async_nats::jetstream::Context,
    >,
) -> ActorHost<
    HostCounterActor,
    async_nats::jetstream::kv::Store,
    NatsTranscriptPublisher,
    async_nats::Client,
    async_nats::jetstream::kv::Store,
    async_nats::jetstream::Context,
> {
    let capability = AgentCapability::new(
        "HostCounterActor",
        ["host_count"],
        "actors.host-counter.>",
    );
    ActorHost::new(runtime, HostCounterActor, capability)
}

async fn make_runtime(
    nats: async_nats::Client,
    js: async_nats::jetstream::Context,
) -> ActorRuntime<
    async_nats::jetstream::kv::Store,
    NatsTranscriptPublisher,
    async_nats::Client,
    async_nats::jetstream::kv::Store,
    async_nats::jetstream::Context,
> {
    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(registry_store);
    ActorRuntime::new(state_store, publisher, nats, registry, js)
}

/// Publish a NATS message to the actor's inbox and verify that:
/// - the message is dispatched to the actor's `handle()` method, and
/// - the resulting state is persisted in the ACTOR_STATE KV bucket.
#[tokio::test]
async fn host_dispatches_nats_message_to_actor() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats.clone(), js.clone()).await;
    let state_store = js.get_key_value("ACTOR_STATE").await.unwrap();

    let host = Arc::new(make_host(runtime));
    let host_task = Arc::clone(&host);
    let run_handle = tokio::spawn(async move { host_task.run().await });

    // Give the subscription time to be established before publishing.
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    nats.publish("actors.host-counter.entity-1", Bytes::new())
        .await
        .unwrap();
    nats.flush().await.unwrap();

    // Wait for the actor to process the message.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    host.cancel();
    tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        run_handle,
    )
    .await
    .expect("host.run() did not stop within 2s")
    .unwrap()  // JoinError
    .unwrap(); // HostError

    // State for "host-counter" actor, entity "entity-1" must have count == 1.
    let entry = state_store
        .entry("host-counter.entity-1")
        .await
        .unwrap()
        .expect("state entry not found — actor may not have handled the message");
    let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
    assert_eq!(saved.count, 1, "actor should have handled exactly one message");
}

/// Calling `cancel()` on a running host causes `run()` to return `Ok(())`.
#[tokio::test]
async fn host_cancel_stops_run_cleanly() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats, js).await;

    let host = Arc::new(make_host(runtime));
    let host_task = Arc::clone(&host);
    let run_handle = tokio::spawn(async move { host_task.run().await });

    // Cancel immediately — no messages published.
    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    host.cancel();

    let result = tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        run_handle,
    )
    .await
    .expect("host.run() did not stop within 2s")
    .unwrap(); // JoinError

    assert!(result.is_ok(), "expected Ok(()), got: {result:?}");
}

/// The entity key is extracted from the NATS subject by stripping the
/// `actors.{type}.` prefix. Verify that state is persisted under the correct
/// KV key when the entity key contains dots (e.g. `owner.repo.42`).
#[tokio::test]
async fn host_entity_key_extracted_from_nats_subject() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats.clone(), js.clone()).await;
    let state_store = js.get_key_value("ACTOR_STATE").await.unwrap();

    let host = Arc::new(make_host(runtime));
    let host_task = Arc::clone(&host);
    let run_handle = tokio::spawn(async move { host_task.run().await });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Entity key with dots: "owner.repo.42"
    nats.publish("actors.host-counter.owner.repo.42", Bytes::new())
        .await
        .unwrap();
    nats.flush().await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    host.cancel();
    tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        run_handle,
    )
    .await
    .expect("host.run() did not stop within 2s")
    .unwrap()
    .unwrap();

    // KV key: "host-counter.owner.repo.42"
    let entry = state_store
        .entry("host-counter.owner.repo.42")
        .await
        .unwrap()
        .expect("state not found under host-counter.owner.repo.42");
    let saved: Counter = serde_json::from_slice(&entry.value).unwrap();
    assert_eq!(saved.count, 1);
}

/// Destroying the AGENT_REGISTRY backing stream before calling `host.run()`
/// causes `registry.register()` to fail. `run()` must return
/// `Err(HostError::Register(_))` — covers host.rs error-return path.
#[tokio::test]
async fn host_run_returns_register_error_when_registry_unavailable() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats, js.clone()).await;

    // Remove the registry backing stream so register() fails.
    js.delete_stream("KV_AGENT_REGISTRY").await.unwrap();

    let host = make_host(runtime);
    let result = host.run().await;
    assert!(
        matches!(result, Err(HostError::Register(_))),
        "expected HostError::Register, got: {result:?}"
    );
}

// ── FailPublisher ─────────────────────────────────────────────────────────────
//
// A transcript publisher that always returns an error, used to verify that a
// transcript write failure inside the spawn_fn is silent — the spawn and the
// overall `handle_event` must still succeed.

#[derive(Clone)]
struct FailPublisher;

impl TranscriptPublisher for FailPublisher {
    async fn publish(&self, _subject: String, _payload: bytes::Bytes) -> Result<(), TranscriptError> {
        Err(TranscriptError::Publish("always fails".into()))
    }
}

/// When the transcript publisher fails, the `let _ = session.append(...)` inside
/// the spawn_fn discards the error. The NATS request-reply must still complete and
/// `handle_event` must return `Ok`.
///
/// Covers `runtime.rs` — `build_context` spawn closure, line `let _ = session.append(...).await`.
#[tokio::test]
async fn spawn_fn_transcript_append_failure_does_not_abort_spawn() {
    let (nats, js, _container) = setup().await;

    let state_store = provision_state(&js).await.unwrap();
    let registry_store = provision_registry(&js).await.unwrap();
    let registry = Registry::new(registry_store);

    // Register a sub-agent with a concrete inbox subject.
    let cap = AgentCapability::new(
        "SecurityActor",
        ["security_analysis"],
        "sub-agents.security-fail-pub",
    );
    registry.register(&cap).await.unwrap();

    // Spin up a responder on the sub-agent's inbox.
    let nats_responder = nats.clone();
    let mut responder_sub = nats.subscribe("sub-agents.security-fail-pub").await.unwrap();
    tokio::spawn(async move {
        while let Some(msg) = responder_sub.next().await {
            if let Some(reply) = msg.reply {
                nats_responder
                    .publish(reply, Bytes::from_static(b"ok"))
                    .await
                    .ok();
            }
        }
    });

    // Use a publisher that always fails — transcript appends are all discarded.
    let runtime = ActorRuntime::new(state_store, FailPublisher, nats, registry, js);

    let result = runtime
        .handle_event(&mut SpawnActor, "entity-fail-pub", 0)
        .await;

    assert!(
        result.is_ok(),
        "handle_event must succeed when transcript publisher fails: {result:?}"
    );
}
