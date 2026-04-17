//! NATS KV integration tests for [`SessionStore`].
//!
//! Each test spins up a fresh NATS container so there is no state leakage.

use async_nats::jetstream;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_agent::{
    agent_loop::{ContentBlock, Message},
    session::{ChatSession, SessionStore},
};

async fn make_store() -> (SessionStore, impl Drop) {
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
    let store = SessionStore::open(&js).await.expect("SessionStore::open");
    (store, container)
}

fn sample_session(id: &str, tenant_id: &str) -> ChatSession {
    ChatSession {
        id: id.to_string(),
        tenant_id: tenant_id.to_string(),
        name: format!("Session {id}"),
        model: None,
        tools: vec![],
        memory_path: None,
        messages: vec![
            Message::user_text("Hello"),
            Message::assistant(vec![ContentBlock::Text {
                text: "Hi!".to_string(),
            }]),
        ],
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
        started_at_secs: 0,
        duration_ms: 0,
        agent_id: None,
    }
}

// ── put / get ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn put_and_get_round_trips() {
    let (store, _c) = make_store().await;
    let s = sample_session("sess-1", "acme");
    store.put(&s).await.expect("put");

    let got = store
        .get("acme", "sess-1")
        .await
        .expect("get")
        .expect("should exist");
    assert_eq!(got.id, "sess-1");
    assert_eq!(got.tenant_id, "acme");
    assert_eq!(got.name, "Session sess-1");
    assert_eq!(got.messages.len(), 2);
    assert_eq!(got.messages[0].role, "user");
    assert_eq!(got.messages[1].role, "assistant");
}

#[tokio::test]
async fn get_missing_session_returns_none() {
    let (store, _c) = make_store().await;
    let result = store.get("acme", "does-not-exist").await.expect("get");
    assert!(result.is_none());
}

#[tokio::test]
async fn put_overwrites_existing_session() {
    let (store, _c) = make_store().await;
    let mut s = sample_session("sess-1", "acme");
    store.put(&s).await.expect("put v1");

    s.name = "Renamed".to_string();
    store.put(&s).await.expect("put v2");

    let got = store.get("acme", "sess-1").await.expect("get").unwrap();
    assert_eq!(got.name, "Renamed");
}

// ── list ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_returns_all_sessions_for_tenant() {
    let (store, _c) = make_store().await;
    store.put(&sample_session("s1", "acme")).await.unwrap();
    store.put(&sample_session("s2", "acme")).await.unwrap();
    store.put(&sample_session("s3", "acme")).await.unwrap();

    let list = store.list("acme").await.expect("list");
    assert_eq!(list.len(), 3);
}

#[tokio::test]
async fn list_empty_when_no_sessions() {
    let (store, _c) = make_store().await;
    let list = store.list("acme").await.expect("list");
    assert!(list.is_empty());
}

#[tokio::test]
async fn list_tenant_isolation() {
    let (store, _c) = make_store().await;
    store.put(&sample_session("s1", "acme")).await.unwrap();
    store.put(&sample_session("s2", "acme")).await.unwrap();
    store.put(&sample_session("s3", "other")).await.unwrap();

    let acme = store.list("acme").await.expect("acme list");
    let other = store.list("other").await.expect("other list");

    assert_eq!(acme.len(), 2);
    assert!(acme.iter().all(|s| s.tenant_id == "acme"));
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].tenant_id, "other");
}

// ── delete ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_removes_session() {
    let (store, _c) = make_store().await;
    store
        .put(&sample_session("sess-del", "acme"))
        .await
        .unwrap();

    // Confirm it exists first.
    assert!(store.get("acme", "sess-del").await.unwrap().is_some());

    store.delete("acme", "sess-del").await.expect("delete");

    // Now it should be gone.
    let after = store.get("acme", "sess-del").await.unwrap();
    assert!(after.is_none());
}

#[tokio::test]
async fn delete_does_not_affect_other_sessions() {
    let (store, _c) = make_store().await;
    store.put(&sample_session("keep", "acme")).await.unwrap();
    store.put(&sample_session("drop", "acme")).await.unwrap();

    store.delete("acme", "drop").await.unwrap();

    let list = store.list("acme").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "keep");
}

// ── message history persistence ────────────────────────────────────────────────

#[tokio::test]
async fn messages_are_preserved_across_put_get() {
    let (store, _c) = make_store().await;
    let mut s = sample_session("sess-msg", "acme");
    s.messages.push(Message::user_text("Follow-up question"));
    store.put(&s).await.unwrap();

    let got = store.get("acme", "sess-msg").await.unwrap().unwrap();
    assert_eq!(got.messages.len(), 3);
    assert_eq!(got.messages[2].role, "user");
}

// ── New field round-trips ─────────────────────────────────────────────────────

#[tokio::test]
async fn session_metadata_fields_round_trip() {
    let (store, _c) = make_store().await;
    let s = ChatSession {
        id: "sess-meta".to_string(),
        tenant_id: "acme".to_string(),
        name: "Meta session".to_string(),
        model: None,
        tools: vec![],
        memory_path: None,
        messages: vec![],
        created_at: "2026-04-17T00:00:00Z".to_string(),
        updated_at: "2026-04-17T00:00:00Z".to_string(),
        started_at_secs: 1776384000,
        duration_ms: 42500,
        agent_id: Some("agent_abc123".to_string()),
    };
    store.put(&s).await.expect("put");

    let got = store.get("acme", "sess-meta").await.expect("get").expect("exists");
    assert_eq!(got.started_at_secs, 1776384000, "started_at_secs must survive KV round-trip");
    assert_eq!(got.duration_ms, 42500,          "duration_ms must survive KV round-trip");
    assert_eq!(got.agent_id, Some("agent_abc123".to_string()), "agent_id must survive KV round-trip");
}

#[tokio::test]
async fn message_usage_round_trips_through_kv() {
    use trogon_agent::agent_loop::{ContentBlock, Usage};

    let (store, _c) = make_store().await;
    let mut s = sample_session("sess-usage", "acme");
    s.messages[1] = Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text { text: "Hi!".to_string() }],
        usage: Some(Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(10),
            cache_read_input_tokens: Some(5),
        }),
    };
    store.put(&s).await.expect("put");

    let got = store.get("acme", "sess-usage").await.expect("get").expect("exists");
    let usage = got.messages[1].usage.as_ref().expect("usage must be present");
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_creation_input_tokens, Some(10));
    assert_eq!(usage.cache_read_input_tokens, Some(5));
}

#[tokio::test]
async fn session_agent_id_none_round_trips() {
    let (store, _c) = make_store().await;
    // Default sample_session has agent_id: None — verify it survives round-trip
    let s = sample_session("sess-no-agent", "acme");
    store.put(&s).await.expect("put");
    let got = store.get("acme", "sess-no-agent").await.expect("get").expect("exists");
    assert_eq!(got.agent_id, None, "agent_id None must round-trip as None");
    assert_eq!(got.started_at_secs, 0, "zero started_at_secs must round-trip");
    assert_eq!(got.duration_ms, 0, "zero duration_ms must round-trip");
}

/// `list()` must silently skip entries whose bytes cannot be deserialized,
/// warning rather than failing, and return the other valid sessions.
#[tokio::test]
async fn list_skips_unreadable_entry_and_returns_valid_ones() {
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
    let store = SessionStore::open(&js).await.expect("open");

    // Insert one valid session.
    store.put(&sample_session("valid-1", "acme")).await.unwrap();

    // Inject corrupted bytes for a second key under the same tenant prefix.
    let kv = js
        .get_key_value(trogon_agent::session::SESSIONS_BUCKET)
        .await
        .expect("get KV bucket");
    kv.put(
        "acme.corrupted",
        bytes::Bytes::from(b"not valid json" as &[u8]),
    )
    .await
    .expect("inject bad bytes");

    // list() must return only the valid session and silently skip the corrupt one.
    let result = store.list("acme").await.expect("list");
    assert_eq!(result.len(), 1, "only the valid session should be returned");
    assert_eq!(result[0].id, "valid-1");
}
