//! Integration tests for the interactive chat session HTTP API.
//!
//! Spins up a real NATS container (for SessionStore KV), a mock Anthropic
//! proxy (httpmock), and an Axum server serving the chat routes, then
//! exercises every endpoint via reqwest.

use std::sync::Arc;

use async_nats::jetstream;
use httpmock::MockServer;
use reqwest::Client;
use serde_json::{Value, json};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ImageExt};
use tokio::net::TcpListener;
use trogon_agent::{
    agent_loop::AgentLoop,
    chat_api::{ChatAppState, router},
    session::SessionStore,
    tools::ToolContext,
};

// ── Setup ─────────────────────────────────────────────────────────────────────

struct TestEnv {
    base_url: String,
    client: Client,
    mock_server: MockServer,
    // Keep container alive for the test duration.
    _nats: Box<dyn std::any::Any>,
}

fn end_turn(text: &str) -> serde_json::Value {
    json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": text }]
    })
}

async fn start() -> TestEnv {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS container");
    let nats_port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);

    let session_store = SessionStore::open(&js).await.expect("SessionStore");

    let mock_server = MockServer::start_async().await;

    let http_client = reqwest::Client::new();
    let agent = Arc::new(AgentLoop {
        http_client: http_client.clone(),
        proxy_url: mock_server.base_url(),
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 5,
        tool_context: Arc::new(ToolContext {
            http_client,
            proxy_url: mock_server.base_url(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
    });

    let state = ChatAppState { agent, session_store };
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });

    TestEnv {
        base_url: format!("http://{addr}"),
        client: Client::new(),
        mock_server,
        _nats: Box::new(container),
    }
}

// ── Missing tenant header ──────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_missing_tenant_returns_400() {
    let env = start().await;
    let res = env.client.get(format!("{}/sessions", env.base_url))
        .send().await.unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn create_session_missing_tenant_returns_400() {
    let env = start().await;
    let res = env.client.post(format!("{}/sessions", env.base_url))
        .json(&json!({"name": "test"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 400);
}

// ── Create session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_session_returns_201_with_id() {
    let env = start().await;
    let res = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "My Agent"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert!(!body["id"].as_str().unwrap_or("").is_empty());
    assert_eq!(body["name"], "My Agent");
    assert_eq!(body["tenant_id"], "acme");
    assert_eq!(body["message_count"], 0);
}

#[tokio::test]
async fn create_session_default_name() {
    let env = start().await;
    let res: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(res["name"], "New Agent");
}

#[tokio::test]
async fn create_session_with_model_and_tools() {
    let env = start().await;
    let res: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "Fast Agent",
            "model": "claude-haiku-4-5-20251001",
            "tools": ["get_pr_diff"]
        }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(res["model"], "claude-haiku-4-5-20251001");
    assert_eq!(res["tools"], json!(["get_pr_diff"]));
}

// ── List sessions ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_empty_returns_empty_array() {
    let env = start().await;
    let res: Value = env.client.get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(res, json!([]));
}

#[tokio::test]
async fn list_sessions_returns_created_sessions() {
    let env = start().await;
    env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "A"}))
        .send().await.unwrap();
    env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "B"}))
        .send().await.unwrap();

    let list: Value = env.client.get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn list_sessions_tenant_isolation() {
    let env = start().await;
    env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "acme session"}))
        .send().await.unwrap();
    env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "other")
        .json(&json!({"name": "other session"}))
        .send().await.unwrap();

    let acme_list: Value = env.client.get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap().json().await.unwrap();
    let other_list: Value = env.client.get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "other")
        .send().await.unwrap().json().await.unwrap();

    assert_eq!(acme_list.as_array().unwrap().len(), 1);
    assert_eq!(other_list.as_array().unwrap().len(), 1);
    assert_ne!(acme_list[0]["id"], other_list[0]["id"]);
}

// ── Get session ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_session_returns_200_with_details() {
    let env = start().await;
    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "My Agent"}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let got: Value = env.client.get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(got["id"], id);
    assert_eq!(got["name"], "My Agent");
    // Full session view includes messages array.
    assert!(got["messages"].is_array());
}

#[tokio::test]
async fn get_session_not_found_returns_404() {
    let env = start().await;
    let res = env.client.get(format!("{}/sessions/does-not-exist", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap();
    assert_eq!(res.status(), 404);
}

// ── Update session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_session_name_model_tools() {
    let env = start().await;
    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "Old Name"}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let updated: Value = env.client.patch(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "New Name",
            "model": "claude-haiku-4-5-20251001",
            "tools": ["get_pr_diff"]
        }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(updated["name"], "New Name");
    assert_eq!(updated["model"], "claude-haiku-4-5-20251001");
}

#[tokio::test]
async fn update_session_not_found_returns_404() {
    let env = start().await;
    let res = env.client.patch(format!("{}/sessions/ghost", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "x"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 404);
}

// ── Delete session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_session_returns_204_and_then_404() {
    let env = start().await;
    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "Doomed"}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let del = env.client.delete(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap();
    assert_eq!(del.status(), 204);

    let get = env.client.get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap();
    assert_eq!(get.status(), 404);
}

#[tokio::test]
async fn delete_session_not_found_returns_404() {
    let env = start().await;
    let res = env.client.delete(format!("{}/sessions/ghost", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap();
    assert_eq!(res.status(), 404);
}

// ── Send message ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn send_message_returns_agent_response() {
    let env = start().await;

    // Mock Anthropic: immediate end_turn.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("Hello from agent"));
    });

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "Hi there"}))
        .send().await.unwrap().json().await.unwrap();

    assert_eq!(res["content"], "Hello from agent");
    assert!(res["message_count"].as_u64().unwrap() >= 2); // user + assistant
}

#[tokio::test]
async fn send_message_auto_names_session_from_first_message() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("ok"));
    });

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["name"], "New Agent");

    env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "What is the capital of France?"}))
        .send().await.unwrap();

    // Name should now be set to the first message content.
    let got: Value = env.client.get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(got["name"], "What is the capital of France?");
}

#[tokio::test]
async fn send_message_history_persisted_for_second_turn() {
    let env = start().await;

    // Both turns return end_turn.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("Paris"));
    });

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    // Turn 1.
    let r1: Value = env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "What is the capital of France?"}))
        .send().await.unwrap().json().await.unwrap();
    let count_after_t1 = r1["message_count"].as_u64().unwrap();

    // Turn 2.
    let r2: Value = env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "And of Germany?"}))
        .send().await.unwrap().json().await.unwrap();
    let count_after_t2 = r2["message_count"].as_u64().unwrap();

    // History must have grown.
    assert!(count_after_t2 > count_after_t1, "second turn must append to history");
    // At least 4 messages: user, assistant, user, assistant.
    assert!(count_after_t2 >= 4);
}

#[tokio::test]
async fn send_message_to_missing_session_returns_404() {
    let env = start().await;
    let res = env.client.post(format!("{}/sessions/ghost/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hi"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn send_message_missing_tenant_returns_400() {
    let env = start().await;
    let res = env.client.post(format!("{}/sessions/any/messages", env.base_url))
        .json(&json!({"content": "hi"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn send_message_with_model_override_uses_override() {
    let env = start().await;

    let mock = env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("claude-haiku-4-5-20251001");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("haiku says hi"));
    });

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"model": "claude-haiku-4-5-20251001"}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hello"}))
        .send().await.unwrap().json().await.unwrap();

    assert_eq!(res["content"], "haiku says hi");
    mock.assert_hits_async(1).await;
}

#[tokio::test]
async fn send_message_anthropic_500_returns_500() {
    let env = start().await;

    // Anthropic proxy always returns 500 — agent gets an HTTP error parsing JSON.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(500).body("Internal Server Error");
    });

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let res = env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hello"}))
        .send().await.unwrap();

    assert_eq!(res.status(), 500, "Anthropic 500 must propagate as HTTP 500");
}

#[tokio::test]
async fn update_session_memory_path_persisted() {
    let env = start().await;

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "MP Session"}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    // Initially no memory_path.
    assert!(created["memory_path"].is_null());

    // PATCH with memory_path.
    let updated: Value = env.client.patch(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"memory_path": "docs/memory.md"}))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(updated["memory_path"], "docs/memory.md");

    // GET to confirm persistence.
    let got: Value = env.client.get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(got["memory_path"], "docs/memory.md");
}

#[tokio::test]
async fn send_message_tools_filtered_by_session_tools() {
    let env = start().await;

    // This mock only matches when the Anthropic body contains "get_pr_diff".
    // If the tool list is correctly filtered, only get_pr_diff should appear.
    let filtered_mock = env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("get_pr_diff");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("diff result"));
    });

    // This mock fires if a non-filtered tool (create_linear_issue) appears — must NOT fire.
    let unfiltered_mock = env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("create_linear_issue");
        then.status(500).body("unexpected tool in body");
    });

    let created: Value = env.client.post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"tools": ["get_pr_diff"]}))
        .send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env.client.post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "show me the diff"}))
        .send().await.unwrap().json().await.unwrap();

    assert_eq!(res["content"], "diff result");
    filtered_mock.assert_hits_async(1).await;
    unfiltered_mock.assert_hits_async(0).await;
}
