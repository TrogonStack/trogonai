use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_actor::{runtime::ActorRuntime, state::provision_state};
use trogon_pr_actor::actor::{PrActor, PrState};
use trogon_registry::{Registry, provision as provision_registry};
use trogon_transcript::publisher::NatsTranscriptPublisher;

async fn setup() -> (
    async_nats::Client,
    async_nats::jetstream::Context,
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
    (nats, js, container)
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

/// The first `handle_event` call on a new entity key creates an entry in the
/// ACTOR_STATE KV bucket with `events_processed == 1`.
#[tokio::test]
async fn pr_actor_first_event_creates_state() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats, js.clone()).await;

    runtime
        .handle_event(&mut PrActor::default(), "acme.repo.1", 0)
        .await
        .unwrap();

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let entry = kv
        .entry("pr.acme.repo.1")
        .await
        .unwrap()
        .expect("state entry missing");
    let state: PrState = serde_json::from_slice(&entry.value).unwrap();
    assert_eq!(state.events_processed, 1);
}

/// Successive events accumulate: `events_processed` increments on every call
/// and the state is persisted between invocations.
#[tokio::test]
async fn pr_actor_state_accumulates_across_events() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats, js.clone()).await;

    for _ in 0..3 {
        runtime
            .handle_event(&mut PrActor::default(), "acme.repo.99", 0)
            .await
            .unwrap();
    }

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();
    let bytes = kv.entry("pr.acme.repo.99").await.unwrap().unwrap().value;
    let state: PrState = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        state.events_processed, 3,
        "all three events must be counted"
    );
}

/// Two distinct entity keys produce independent state entries — they do not
/// share or overwrite each other's `events_processed` counter.
#[tokio::test]
async fn pr_actor_different_entities_have_independent_state() {
    let (nats, js, _container) = setup().await;
    let runtime = make_runtime(nats, js.clone()).await;

    runtime
        .handle_event(&mut PrActor::default(), "acme.repo.10", 0)
        .await
        .unwrap();
    runtime
        .handle_event(&mut PrActor::default(), "acme.repo.10", 0)
        .await
        .unwrap();
    runtime
        .handle_event(&mut PrActor::default(), "acme.repo.20", 0)
        .await
        .unwrap();

    let kv = js.get_key_value("ACTOR_STATE").await.unwrap();

    let bytes_10 = kv.entry("pr.acme.repo.10").await.unwrap().unwrap().value;
    let state_10: PrState = serde_json::from_slice(&bytes_10).unwrap();
    assert_eq!(state_10.events_processed, 2, "pr 10 should have 2 events");

    let bytes_20 = kv.entry("pr.acme.repo.20").await.unwrap().unwrap().value;
    let state_20: PrState = serde_json::from_slice(&bytes_20).unwrap();
    assert_eq!(state_20.events_processed, 1, "pr 20 should have 1 event");
}
