use std::time::Duration;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use trogon_registry::{AgentCapability, Registry, provision};

async fn setup() -> (async_nats::jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("failed to start NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("failed to get port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = async_nats::jetstream::new(nats);
    (js, container)
}

fn pr_actor() -> AgentCapability {
    AgentCapability::new("PrActor", ["code_review", "security_analysis"], "actors.pr.>")
}

fn incident_actor() -> AgentCapability {
    AgentCapability::new(
        "IncidentActor",
        ["triage", "timeline", "escalation"],
        "actors.incident.>",
    )
}

#[tokio::test]
async fn provision_creates_agent_registry_bucket() {
    let (js, _container) = setup().await;

    provision(&js).await.expect("provision failed");

    js.get_key_value("AGENT_REGISTRY")
        .await
        .expect("AGENT_REGISTRY bucket not found after provision");
}

#[tokio::test]
async fn provision_is_idempotent() {
    let (js, _container) = setup().await;

    provision(&js).await.expect("first provision failed");
    provision(&js).await.expect("second provision should not fail");
}

#[tokio::test]
async fn register_appears_in_list_all() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    registry.register(&pr_actor()).await.unwrap();

    let all = registry.list_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].agent_type, "PrActor");
    assert_eq!(all[0].nats_subject, "actors.pr.>");
}

#[tokio::test]
async fn register_multiple_agents_all_visible() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    registry.register(&pr_actor()).await.unwrap();
    registry.register(&incident_actor()).await.unwrap();

    let all = registry.list_all().await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn discover_returns_matching_agents_only() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    registry.register(&pr_actor()).await.unwrap();
    registry.register(&incident_actor()).await.unwrap();

    let found = registry.discover("code_review").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].agent_type, "PrActor");

    let found_triage = registry.discover("triage").await.unwrap();
    assert_eq!(found_triage.len(), 1);
    assert_eq!(found_triage[0].agent_type, "IncidentActor");

    let found_none = registry.discover("unknown_capability").await.unwrap();
    assert!(found_none.is_empty());
}

#[tokio::test]
async fn unregister_removes_entry_immediately() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    registry.register(&pr_actor()).await.unwrap();
    registry.register(&incident_actor()).await.unwrap();

    registry.unregister("PrActor").await.unwrap();

    let all = registry.list_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].agent_type, "IncidentActor");
}

#[tokio::test]
async fn refresh_updates_current_load() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    registry.register(&pr_actor()).await.unwrap();

    let mut updated = pr_actor();
    updated.current_load = 7;
    registry.refresh(&updated).await.unwrap();

    let all = registry.list_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].current_load, 7);
}

#[tokio::test]
async fn list_all_empty_on_fresh_bucket() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    let all = registry.list_all().await.unwrap();
    assert!(all.is_empty());
}

/// Provision a bucket with a very short TTL, register an agent, wait for the
/// TTL to elapse, and verify the entry is no longer visible.
/// This exercises the mechanism by which crashed agents disappear automatically.
#[tokio::test]
async fn entry_expires_after_ttl() {
    let (js, _container) = setup().await;

    // Create a test-only bucket with a 2-second TTL (production uses 30s).
    let short_ttl_store = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "TEST_REGISTRY_TTL".to_string(),
            history: 1,
            max_age: Duration::from_secs(2),
            storage: async_nats::jetstream::stream::StorageType::Memory,
            ..Default::default()
        })
        .await
        .expect("failed to create short-TTL bucket");

    let registry = Registry::new(short_ttl_store);
    registry.register(&pr_actor()).await.unwrap();

    // Entry should be visible immediately.
    assert_eq!(registry.list_all().await.unwrap().len(), 1);

    // Wait for the TTL to expire.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Entry should be gone — no heartbeat was sent.
    let after = registry.list_all().await.unwrap();
    assert!(
        after.is_empty(),
        "agent entry should have expired, but still found: {after:?}"
    );
}

/// Same scenario as entry_expires_after_ttl but verifies via discover().
#[tokio::test]
async fn expired_entry_not_returned_by_discover() {
    let (js, _container) = setup().await;

    let short_ttl_store = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "TEST_REGISTRY_TTL_DISCOVER".to_string(),
            history: 1,
            max_age: Duration::from_secs(2),
            storage: async_nats::jetstream::stream::StorageType::Memory,
            ..Default::default()
        })
        .await
        .unwrap();

    let registry = Registry::new(short_ttl_store);
    registry.register(&pr_actor()).await.unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;

    let found = registry.discover("code_review").await.unwrap();
    assert!(
        found.is_empty(),
        "discover should return nothing after TTL, but found: {found:?}"
    );
}

/// Verify that refresh() resets the TTL: the entry survives past one TTL window
/// if it is refreshed before expiry.
#[tokio::test]
async fn refresh_keeps_entry_alive_past_ttl() {
    let (js, _container) = setup().await;

    let short_ttl_store = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "TEST_REGISTRY_TTL_REFRESH".to_string(),
            history: 1,
            max_age: Duration::from_secs(2),
            storage: async_nats::jetstream::stream::StorageType::Memory,
            ..Default::default()
        })
        .await
        .unwrap();

    let registry = Registry::new(short_ttl_store);
    registry.register(&pr_actor()).await.unwrap();

    // Refresh after 1s — before the 2s TTL fires.
    tokio::time::sleep(Duration::from_secs(1)).await;
    registry.refresh(&pr_actor()).await.unwrap();

    // Another 1s passes (total 2s since registration, but only 1s since refresh).
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Entry should still be alive because we refreshed within the TTL window.
    let all = registry.list_all().await.unwrap();
    assert_eq!(all.len(), 1, "refreshed entry should still be alive");
}

#[tokio::test]
async fn register_overwrites_previous_registration() {
    let (js, _container) = setup().await;
    let store = provision(&js).await.unwrap();
    let registry = Registry::new(store);

    registry.register(&pr_actor()).await.unwrap();

    let mut v2 = pr_actor();
    v2.capabilities.push("dependency_check".to_string());
    v2.current_load = 3;
    registry.register(&v2).await.unwrap();

    let all = registry.list_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].current_load, 3);
    assert!(all[0].has_capability("dependency_check"));
}
