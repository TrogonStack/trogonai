//! Live end-to-end test for trogon-actor: EntityActor trait, ActorRuntime OCC,
//! state persistence in ACTOR_STATE KV, transcript writes, and ActorHost inbox dispatch.
//!
//! Requirements:
//!   • NATS running on nats://localhost:4222  (JetStream required)
//!   • No external credentials — LLM calls are not made
//!
//! Run:
//!   cargo run -p trogon-e2e --bin actor_live

use std::time::Duration;

use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use trogon_actor::{
    ActorContext, ActorRuntime, EntityActor, StateStore,
    host::ActorHost,
    provision_state, state_kv_key,
};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_registry::{AgentCapability, Registry, provision as provision_registry};
use trogon_transcript::{NatsTranscriptPublisher, TranscriptStore};
use uuid::Uuid;

// ── Output helpers ─────────────────────────────────────────────────────────────

fn ok(label: &str) {
    println!("  \x1b[32m✓\x1b[0m  {label}");
}

fn ko(label: &str, reason: &str) {
    println!("  \x1b[31m✗\x1b[0m  {label}");
    println!("       {reason}");
}

fn uid() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

// ── Test actor ────────────────────────────────────────────────────────────────

#[derive(Default, Clone, Serialize, Deserialize)]
struct CounterState {
    count: u32,
    last_message: String,
}

#[derive(Clone)]
struct CounterActor;

impl EntityActor for CounterActor {
    type State = CounterState;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "live-counter"
    }

    async fn handle(
        &mut self,
        state: &mut CounterState,
        ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.count += 1;
        state.last_message = format!("event #{}", state.count);
        ctx.append_user_message(format!("handling event, count now {}", state.count), None)
            .await
            .ok();
        ctx.append_assistant_message("acknowledged", None).await.ok();
        Ok(())
    }
}

// ── Runtime builder ────────────────────────────────────────────────────────────

async fn make_runtime(
    js: &jetstream::Context,
    nats: &async_nats::Client,
) -> ActorRuntime<
    async_nats::jetstream::kv::Store,
    NatsTranscriptPublisher,
    async_nats::Client,
    async_nats::jetstream::kv::Store,
    NatsJetStreamClient,
> {
    let state_store = provision_state(js).await.unwrap();
    let reg_store = provision_registry(js).await.unwrap();
    let publisher = NatsTranscriptPublisher::new(js.clone());
    let registry = Registry::new(reg_store);
    let js_client = NatsJetStreamClient::new(js.clone());
    ActorRuntime::new(state_store, publisher, nats.clone(), registry, js_client)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

async fn test_runtime_saves_state_to_kv(js: &jetstream::Context, nats: &async_nats::Client) -> bool {
    const LABEL: &str = "ActorRuntime — handle_event persists state to ACTOR_STATE KV";
    let entity_key = format!("entity-{}", uid());

    let result: Result<(), String> = async {
        let runtime = make_runtime(js, nats).await;
        let state_store = provision_state(js).await.map_err(|e| e.to_string())?;

        let mut actor = CounterActor;
        runtime.handle_event(&mut actor, &entity_key, 0).await.map_err(|e| format!("{e:?}"))?;

        let kv_key = state_kv_key(CounterActor::actor_type(), &entity_key);
        let entry = state_store
            .load(&kv_key)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no state found at key '{kv_key}'"))?;

        let state: CounterState = serde_json::from_slice(&entry.value).map_err(|e| e.to_string())?;
        if state.count != 1 {
            return Err(format!("expected count=1 after first event, got {}", state.count));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_runtime_state_accumulates_across_events(js: &jetstream::Context, nats: &async_nats::Client) -> bool {
    const LABEL: &str = "ActorRuntime — repeated handle_event accumulates state (load-modify-save)";
    let entity_key = format!("accum-{}", uid());

    let result: Result<(), String> = async {
        let runtime = make_runtime(js, nats).await;
        let state_store = provision_state(js).await.map_err(|e| e.to_string())?;

        let mut actor = CounterActor;
        for _ in 0..3 {
            runtime.handle_event(&mut actor, &entity_key, 0).await.map_err(|e| format!("{e:?}"))?;
        }

        let kv_key = state_kv_key(CounterActor::actor_type(), &entity_key);
        let entry = state_store.load(&kv_key).await.map_err(|e| e.to_string())?.ok_or("no state")?;
        let state: CounterState = serde_json::from_slice(&entry.value).map_err(|e| e.to_string())?;
        if state.count != 3 {
            return Err(format!("expected count=3 after three events, got {}", state.count));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_runtime_writes_transcript(js: &jetstream::Context, nats: &async_nats::Client) -> bool {
    const LABEL: &str = "ActorRuntime — handle_event writes transcript entries to TRANSCRIPTS stream";
    let entity_key = format!("transcript-{}", uid());

    let result: Result<(), String> = async {
        let runtime = make_runtime(js, nats).await;

        // Provision the TRANSCRIPTS stream before writing
        let ts = TranscriptStore::new(js.clone());
        ts.provision().await.map_err(|e| e.to_string())?;

        let mut actor = CounterActor;
        runtime.handle_event(&mut actor, &entity_key, 0).await.map_err(|e| format!("{e:?}"))?;

        tokio::time::sleep(Duration::from_millis(150)).await;

        let entries = ts.query(CounterActor::actor_type(), &entity_key).await.map_err(|e| e.to_string())?;
        if entries.len() < 2 {
            return Err(format!("expected ≥2 transcript entries (user+assistant), got {}", entries.len()));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_runtime_occ_two_concurrent_events(js: &jetstream::Context, nats: &async_nats::Client) -> bool {
    const LABEL: &str = "ActorRuntime — two concurrent handle_event calls on same entity both succeed via OCC retry";
    let entity_key = format!("occ-{}", uid());

    let result: Result<(), String> = async {
        // Build two independent runtimes sharing the same ACTOR_STATE KV bucket
        let r1 = make_runtime(js, nats).await;
        let r2 = make_runtime(js, nats).await;

        let mut a1 = CounterActor;
        let mut a2 = CounterActor;

        // Run both concurrently — one will hit an OCC conflict and retry
        let (res1, res2) = tokio::join!(
            r1.handle_event(&mut a1, &entity_key, 0),
            r2.handle_event(&mut a2, &entity_key, 0),
        );

        res1.map_err(|e| format!("runtime 1 failed: {e:?}"))?;
        res2.map_err(|e| format!("runtime 2 failed: {e:?}"))?;

        // Both succeeded — state should be count=2 (both incremented)
        let state_store = provision_state(js).await.map_err(|e| e.to_string())?;
        let kv_key = state_kv_key(CounterActor::actor_type(), &entity_key);
        let entry = state_store.load(&kv_key).await.map_err(|e| e.to_string())?.ok_or("no state")?;
        let state: CounterState = serde_json::from_slice(&entry.value).map_err(|e| e.to_string())?;
        if state.count != 2 {
            return Err(format!("expected count=2 after two concurrent events, got {}", state.count));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_actor_host_registers_in_registry(js: &jetstream::Context, nats: &async_nats::Client) -> bool {
    const LABEL: &str = "ActorHost — run() registers actor in live registry";
    let id = uid();
    let subject = format!("actors.live-counter.host-{id}.>");

    let result: Result<(), String> = async {
        let runtime = make_runtime(js, nats).await;
        let reg_store = provision_registry(js).await.map_err(|e| e.to_string())?;
        let registry = Registry::new(reg_store);

        let cap = AgentCapability::new(
            format!("live-counter-host-{id}"),
            ["counting"],
            &subject,
        );
        let host = ActorHost::new(runtime, CounterActor, cap.clone());

        // Run host in background and give it time to register
        let host_handle = tokio::spawn(async move { host.run().await });
        tokio::time::sleep(Duration::from_millis(200)).await;

        let found = registry
            .get(&cap.agent_type)
            .await
            .map_err(|e| e.to_string())?;

        host_handle.abort();

        found.ok_or_else(|| format!("actor {} not found in registry after host.run()", cap.agent_type))?;
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_actor_host_processes_inbox_message(js: &jetstream::Context, nats: &async_nats::Client) -> bool {
    const LABEL: &str = "ActorHost — publishes NATS message to inbox → actor handles + state saved";
    let id = uid();
    // subject without extra host segment so entity_key strips cleanly to just the entity id
    let subject = format!("actors.live-counter.>");
    let entity_key = format!("inbox-entity-{id}");
    let inbox_subject = format!("actors.live-counter.{entity_key}");

    let result: Result<(), String> = async {
        let runtime = make_runtime(js, nats).await;
        let state_store = provision_state(js).await.map_err(|e| e.to_string())?;

        let cap = AgentCapability::new(
            format!("live-counter-host3-{id}"),
            ["counting"],
            &subject,
        );
        let host = ActorHost::new(runtime, CounterActor, cap);

        let nats_for_publish = nats.clone();
        let host_handle = tokio::spawn(async move { host.run().await });

        // Wait for the host to subscribe to its inbox
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Publish a raw message to the actor's inbox subject
        nats_for_publish
            .publish(inbox_subject.clone(), bytes::Bytes::new())
            .await
            .map_err(|e| e.to_string())?;

        // Wait for the actor to process the event
        tokio::time::sleep(Duration::from_millis(300)).await;

        host_handle.abort();

        // Verify state was saved
        let kv_key = state_kv_key(CounterActor::actor_type(), &entity_key);
        let entry = state_store
            .load(&kv_key)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no state saved for entity '{entity_key}'"))?;
        let state: CounterState = serde_json::from_slice(&entry.value).map_err(|e| e.to_string())?;
        if state.count != 1 {
            return Err(format!("expected count=1 after inbox event, got {}", state.count));
        }
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ══════════════════════════════════════════════════════════════════════════════
// Entry point
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    println!();
    println!("══════════════════════════════════════════════════════════");
    println!(" trogon-actor — EntityActor + ActorRuntime + ActorHost live test");
    println!("  NATS: nats://localhost:4222  (JetStream required)");
    println!("══════════════════════════════════════════════════════════");
    println!();

    let nats = async_nats::connect("nats://localhost:4222")
        .await
        .expect("NATS must be running on localhost:4222 — start it with: nats-server -js");
    let js = jetstream::new(nats.clone());

    println!("ActorRuntime");
    let r1 = test_runtime_saves_state_to_kv(&js, &nats).await;
    let r2 = test_runtime_state_accumulates_across_events(&js, &nats).await;
    let r3 = test_runtime_writes_transcript(&js, &nats).await;
    let r4 = test_runtime_occ_two_concurrent_events(&js, &nats).await;

    println!();
    println!("ActorHost");
    let r5 = test_actor_host_registers_in_registry(&js, &nats).await;
    let r6 = test_actor_host_processes_inbox_message(&js, &nats).await;

    let results = [r1, r2, r3, r4, r5, r6];
    let passed = results.iter().filter(|&&r| r).count();
    let total = results.len();

    println!();
    println!("══════════════════════════════════════════════════════════");
    if passed == total {
        println!(" \x1b[32mAll {total} tests passed\x1b[0m");
    } else {
        println!(" \x1b[31m{passed}/{total} tests passed\x1b[0m");
    }
    println!("══════════════════════════════════════════════════════════");
    println!();

    if passed < total {
        std::process::exit(1);
    }
}
