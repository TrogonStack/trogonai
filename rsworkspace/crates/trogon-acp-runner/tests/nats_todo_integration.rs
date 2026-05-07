//! Integration tests for `NatsTodoTool` — requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test nats_todo_integration --features test-helpers

use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use trogon_acp_runner::nats_todo_tool::NatsTodoTool;
use trogon_acp_runner::session_store::NatsSessionStore;
use trogon_acp_runner::{SessionState, SessionStore};
use trogon_mcp::McpCallTool;

async fn setup() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (container, nats, js)
}

// ── todo_write ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn todo_write_persists_item_in_nats_kv() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-1", store.clone());

    let result = tool
        .call_tool(
            "todo_write",
            &serde_json::json!({"id": "t1", "content": "implement feature", "status": "pending"}),
        )
        .await
        .unwrap();
    assert_eq!(result, "OK");

    let state = store.load("sess-1").await.unwrap();
    assert_eq!(state.todos.len(), 1);
    assert_eq!(state.todos[0].id, "t1");
    assert_eq!(state.todos[0].content, "implement feature");
    assert_eq!(state.todos[0].status, "pending");
}

#[tokio::test]
async fn todo_write_updates_existing_item_in_nats_kv() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-2", store.clone());

    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t1", "content": "initial", "status": "pending"}),
    )
    .await
    .unwrap();
    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t1", "content": "updated", "status": "in_progress"}),
    )
    .await
    .unwrap();

    let state = store.load("sess-2").await.unwrap();
    assert_eq!(state.todos.len(), 1);
    assert_eq!(state.todos[0].content, "updated");
    assert_eq!(state.todos[0].status, "in_progress");
}

#[tokio::test]
async fn todo_write_multiple_items_stored_separately() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-3", store.clone());

    for (id, content) in &[("t1", "task one"), ("t2", "task two"), ("t3", "task three")] {
        tool.call_tool(
            "todo_write",
            &serde_json::json!({"id": id, "content": content, "status": "pending"}),
        )
        .await
        .unwrap();
    }

    let state = store.load("sess-3").await.unwrap();
    assert_eq!(state.todos.len(), 3);
}

#[tokio::test]
async fn todo_write_rejects_invalid_status() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-4", store.clone());

    let result = tool
        .call_tool(
            "todo_write",
            &serde_json::json!({"id": "t1", "content": "x", "status": "bogus"}),
        )
        .await;
    assert!(result.is_err(), "invalid status must return Err");
}

// ── todo_read ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn todo_read_returns_only_active_todos() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-5", store.clone());

    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t1", "content": "pending task", "status": "pending"}),
    )
    .await
    .unwrap();
    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t2", "content": "in progress", "status": "in_progress"}),
    )
    .await
    .unwrap();
    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t3", "content": "done task", "status": "completed"}),
    )
    .await
    .unwrap();

    let result = tool
        .call_tool("todo_read", &serde_json::json!({}))
        .await
        .unwrap();

    assert!(result.contains("t1"), "pending must appear, got: {result}");
    assert!(result.contains("t2"), "in_progress must appear, got: {result}");
    assert!(!result.contains("t3"), "completed must be hidden, got: {result}");
}

#[tokio::test]
async fn todo_read_empty_session_returns_no_active_message() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-6", store.clone());

    let result = tool
        .call_tool("todo_read", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result, "No active todos.");
}

#[tokio::test]
async fn todo_read_after_all_completed_returns_no_active_message() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-7", store.clone());

    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t1", "content": "done", "status": "completed"}),
    )
    .await
    .unwrap();

    let result = tool
        .call_tool("todo_read", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result, "No active todos.");
}

// ── session isolation ─────────────────────────────────────────────────────────

#[tokio::test]
async fn todos_are_isolated_per_session() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    let tool_a = NatsTodoTool::new("sess-a", store.clone());
    let tool_b = NatsTodoTool::new("sess-b", store.clone());

    tool_a
        .call_tool(
            "todo_write",
            &serde_json::json!({"id": "t1", "content": "session A task", "status": "pending"}),
        )
        .await
        .unwrap();

    let result_b = tool_b
        .call_tool("todo_read", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        result_b, "No active todos.",
        "session B must not see session A's todos"
    );

    let state_a = store.load("sess-a").await.unwrap();
    assert_eq!(state_a.todos.len(), 1);
    let state_b = store.load("sess-b").await.unwrap();
    assert_eq!(state_b.todos.len(), 0);
}

// ── todos survive reload ──────────────────────────────────────────────────────

#[tokio::test]
async fn todos_persist_across_store_reload() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    let tool = NatsTodoTool::new("sess-8", store.clone());

    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t1", "content": "durable task", "status": "pending"}),
    )
    .await
    .unwrap();

    // Open a second handle to the same KV bucket — simulates a restart.
    let store2 = NatsSessionStore::open(&js).await.unwrap();
    let state = store2.load("sess-8").await.unwrap();
    assert_eq!(state.todos.len(), 1);
    assert_eq!(state.todos[0].id, "t1");
}

// ── coexists with other session fields ────────────────────────────────────────

#[tokio::test]
async fn todos_coexist_with_other_session_state_fields() {
    let (_c, _nats, js) = setup().await;
    let store = NatsSessionStore::open(&js).await.unwrap();

    // Save some session state first.
    store
        .save(
            "sess-9",
            &SessionState {
                mode: "plan".to_string(),
                cwd: "/project".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let tool = NatsTodoTool::new("sess-9", store.clone());
    tool.call_tool(
        "todo_write",
        &serde_json::json!({"id": "t1", "content": "task", "status": "pending"}),
    )
    .await
    .unwrap();

    let state = store.load("sess-9").await.unwrap();
    assert_eq!(state.mode, "plan", "existing fields must be preserved");
    assert_eq!(state.cwd, "/project");
    assert_eq!(state.todos.len(), 1);
}
