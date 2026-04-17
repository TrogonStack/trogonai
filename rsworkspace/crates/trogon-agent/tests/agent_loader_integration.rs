//! Integration tests for [`AgentLoader`] against a real NATS KV store.
//!
//! Each test spins up a fresh NATS container so there is no state leakage.

use async_nats::jetstream;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_agent::agent_loader::{AgentLoader, AgentLoading};

const CONSOLE_AGENTS_BUCKET: &str = "CONSOLE_AGENTS";

async fn make_loader() -> (AgentLoader, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = jetstream::new(nats);
    let loader = AgentLoader::open(&js).await.expect("AgentLoader::open");
    (loader, container)
}

async fn make_loader_with_js() -> (AgentLoader, jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = jetstream::new(nats);
    let loader = AgentLoader::open(&js).await.expect("AgentLoader::open");
    (loader, js, container)
}

// ── Returns correct skill_ids ─────────────────────────────────────────────────

#[tokio::test]
async fn agent_loader_returns_skill_ids_from_kv() {
    let (loader, js, _c) = make_loader_with_js().await;

    let kv = js
        .get_key_value(CONSOLE_AGENTS_BUCKET)
        .await
        .expect("get KV bucket");

    let agent_json = serde_json::json!({
        "id": "agent_abc",
        "name": "Test Agent",
        "skill_ids": ["skill_pdf", "skill_search"]
    });
    kv.put(
        "agent_abc",
        bytes::Bytes::from(serde_json::to_vec(&agent_json).unwrap()),
    )
    .await
    .expect("put agent");

    let ids = loader.get_skill_ids("agent_abc").await;
    assert_eq!(ids, vec!["skill_pdf", "skill_search"]);
}

// ── Missing agent returns empty ────────────────────────────────────────────────

#[tokio::test]
async fn agent_loader_returns_empty_for_missing_agent() {
    let (loader, _c) = make_loader().await;
    let ids = loader.get_skill_ids("does_not_exist").await;
    assert!(ids.is_empty(), "missing agent must return empty skill_ids; got {ids:?}");
}

// ── Malformed JSON returns empty ───────────────────────────────────────────────

#[tokio::test]
async fn agent_loader_returns_empty_for_malformed_json() {
    let (loader, js, _c) = make_loader_with_js().await;

    let kv = js
        .get_key_value(CONSOLE_AGENTS_BUCKET)
        .await
        .expect("get KV bucket");

    kv.put(
        "bad_agent",
        bytes::Bytes::from(b"not valid json" as &[u8]),
    )
    .await
    .expect("put bad bytes");

    let ids = loader.get_skill_ids("bad_agent").await;
    assert!(ids.is_empty(), "malformed JSON must return empty skill_ids; got {ids:?}");
}

// ── Missing skill_ids field returns empty ─────────────────────────────────────

#[tokio::test]
async fn agent_loader_returns_empty_when_skill_ids_field_missing() {
    let (loader, js, _c) = make_loader_with_js().await;

    let kv = js
        .get_key_value(CONSOLE_AGENTS_BUCKET)
        .await
        .expect("get KV bucket");

    let agent_json = serde_json::json!({
        "id": "agent_no_skills",
        "name": "Agent Without Skills"
    });
    kv.put(
        "agent_no_skills",
        bytes::Bytes::from(serde_json::to_vec(&agent_json).unwrap()),
    )
    .await
    .expect("put agent");

    let ids = loader.get_skill_ids("agent_no_skills").await;
    assert!(ids.is_empty(), "agent without skill_ids field must return empty; got {ids:?}");
}
