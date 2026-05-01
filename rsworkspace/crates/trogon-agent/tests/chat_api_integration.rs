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
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use tokio::net::TcpListener;
use trogon_agent::{
    agent_loader::AgentLoading,
    agent_loop::{AgentLoop, ReqwestAnthropicClient},
    chat_api::{ChatAppState, router},
    flag_client::AlwaysOnFlagClient,
    promise_store::{AgentPromise, PromiseEntry, PromiseRepository, PromiseStoreError},
    session::SessionStore,
    skill_loader::SkillLoading,
    tools::{DefaultToolDispatcher, ToolContext},
};

// ── Inline loader stubs for skill-injection tests ─────────────────────────────

struct FixedAgentLoader {
    target_id: String,
    skill_ids: Vec<String>,
}

impl AgentLoading for FixedAgentLoader {
    fn get_skill_ids<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send + 'a>> {
        let ids = if agent_id == self.target_id {
            self.skill_ids.clone()
        } else {
            vec![]
        };
        Box::pin(std::future::ready(ids))
    }
}

struct FixedSkillLoader {
    content: Option<String>,
}

impl SkillLoading for FixedSkillLoader {
    fn load<'a>(
        &'a self,
        _skill_ids: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        let c = self.content.clone();
        Box::pin(std::future::ready(c))
    }
}

// Minimal no-op promise store for integration tests — these tests exercise chat
// session routes that don't use promise state.
struct NoOpPromiseStore;

impl PromiseRepository for NoOpPromiseStore {
    fn get_promise<'a>(
        &'a self,
        _tenant_id: &'a str,
        _promise_id: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(None) })
    }

    fn put_promise<'a>(
        &'a self,
        _promise: &'a AgentPromise,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(1) })
    }

    fn update_promise<'a>(
        &'a self,
        _tenant_id: &'a str,
        _promise_id: &'a str,
        _promise: &'a AgentPromise,
        _revision: u64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(1) })
    }

    fn get_tool_result<'a>(
        &'a self,
        _tenant_id: &'a str,
        _promise_id: &'a str,
        _cache_key: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a,
        >,
    > {
        Box::pin(async { Ok(None) })
    }

    fn put_tool_result<'a>(
        &'a self,
        _tenant_id: &'a str,
        _promise_id: &'a str,
        _cache_key: &'a str,
        _result: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PromiseStoreError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }

    fn list_running<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(vec![]) })
    }
}

// ── Response helpers ──────────────────────────────────────────────────────────

fn end_turn_with_usage(text: &str, input_tokens: u32, output_tokens: u32) -> serde_json::Value {
    json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": text }],
        "usage": { "input_tokens": input_tokens, "output_tokens": output_tokens }
    })
}

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
    start_with_options(None, None, None).await
}

async fn start_with_options(
    agent_id: Option<String>,
    agent_loader: Option<Arc<dyn AgentLoading>>,
    skill_loader: Option<Arc<dyn SkillLoading>>,
) -> TestEnv {
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
    let tool_ctx = Arc::new(ToolContext::new(
        http_client.clone(),
        mock_server.base_url(),
        "tok_github_prod_test01".to_string(),
        "tok_linear_prod_test01".to_string(),
        String::new(),
    ));
    let agent = Arc::new(AgentLoop {
        anthropic_client: Arc::new(ReqwestAnthropicClient::new(
            http_client,
            mock_server.base_url(),
            "tok_anthropic_prod_test01".to_string(),
        )),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 5,
        tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
        tool_context: tool_ctx,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        flag_client: Arc::new(AlwaysOnFlagClient),
        tenant_id: "test".to_string(),
        promise_store: None,
        promise_id: None,
        permission_checker: None,
        elicitation_provider: None,
    });

    let state = ChatAppState {
        agent,
        session_store,
        promise_store: Arc::new(NoOpPromiseStore),
        agent_id,
        agent_loader,
        skill_loader,
        approval_registry: Default::default(),
        elicitation_registry: Default::default(),
    };
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
    let res = env
        .client
        .get(format!("{}/sessions", env.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn create_session_missing_tenant_returns_400() {
    let env = start().await;
    let res = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .json(&json!({"name": "test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

// ── Create session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_session_returns_201_with_id() {
    let env = start().await;
    let res = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "My Agent"}))
        .send()
        .await
        .unwrap();
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
    let res: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["name"], "New Agent");
}

#[tokio::test]
async fn create_session_with_model_and_tools() {
    let env = start().await;
    let res: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "Fast Agent",
            "model": "claude-haiku-4-5-20251001",
            "tools": ["get_pr_diff"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["model"], "claude-haiku-4-5-20251001");
    assert_eq!(res["tools"], json!(["get_pr_diff"]));
}

// ── List sessions ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_empty_returns_empty_array() {
    let env = start().await;
    let res: Value = env
        .client
        .get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res, json!([]));
}

#[tokio::test]
async fn list_sessions_returns_created_sessions() {
    let env = start().await;
    env.client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "A"}))
        .send()
        .await
        .unwrap();
    env.client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "B"}))
        .send()
        .await
        .unwrap();

    let list: Value = env
        .client
        .get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn list_sessions_tenant_isolation() {
    let env = start().await;
    env.client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "acme session"}))
        .send()
        .await
        .unwrap();
    env.client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "other")
        .json(&json!({"name": "other session"}))
        .send()
        .await
        .unwrap();

    let acme_list: Value = env
        .client
        .get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_list: Value = env
        .client
        .get(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "other")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(acme_list.as_array().unwrap().len(), 1);
    assert_eq!(other_list.as_array().unwrap().len(), 1);
    assert_ne!(acme_list[0]["id"], other_list[0]["id"]);
}

// ── Get session ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_session_returns_200_with_details() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "My Agent"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["id"], id);
    assert_eq!(got["name"], "My Agent");
    // Full session view includes messages array.
    assert!(got["messages"].is_array());
}

#[tokio::test]
async fn get_session_not_found_returns_404() {
    let env = start().await;
    let res = env
        .client
        .get(format!("{}/sessions/does-not-exist", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// ── Update session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_session_name_model_tools() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "Old Name"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let updated: Value = env
        .client
        .patch(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "New Name",
            "model": "claude-haiku-4-5-20251001",
            "tools": ["get_pr_diff"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["name"], "New Name");
    assert_eq!(updated["model"], "claude-haiku-4-5-20251001");
}

/// PATCH the session tools list, then POST a message — the agent must use the
/// updated tools, not the original ones. Verifies that `send_message` reads
/// the session fresh from KV after the PATCH and applies the new tool filter.
#[tokio::test]
async fn patch_tools_then_send_message_uses_updated_tool_list() {
    let env = start().await;

    // Only fire when "get_pr_diff" is in the Anthropic request (patched tools).
    let patched_mock = env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("get_pr_diff");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("diff result after patch"));
    });

    // Must NOT fire — if create_linear_issue appears, the tool list was not updated.
    let stale_mock = env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("create_linear_issue");
        then.status(500).body("stale tool list detected");
    });

    // Create session with no tools (all tools available by default).
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    // PATCH: restrict tools to only get_pr_diff.
    env.client
        .patch(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"tools": ["get_pr_diff"]}))
        .send()
        .await
        .unwrap();

    // POST a message — must use the patched tool list.
    let res: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "show me the diff"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["content"], "diff result after patch");
    patched_mock.assert_hits_async(1).await;
    stale_mock.assert_hits_async(0).await;
}

#[tokio::test]
async fn update_session_not_found_returns_404() {
    let env = start().await;
    let res = env
        .client
        .patch(format!("{}/sessions/ghost", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// ── Delete session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_session_returns_204_and_then_404() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "Doomed"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let del = env
        .client
        .delete(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);

    let get = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 404);
}

#[tokio::test]
async fn delete_session_not_found_returns_404() {
    let env = start().await;
    let res = env
        .client
        .delete(format!("{}/sessions/ghost", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

// ── Send message ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn send_message_returns_agent_response() {
    let env = start().await;

    // Mock Anthropic: immediate end_turn.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("Hello from agent"));
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "Hi there"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["content"], "Hello from agent");
    assert!(res["message_count"].as_u64().unwrap() >= 2); // user + assistant
}

#[tokio::test]
async fn send_message_auto_names_session_from_first_message() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("ok"));
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["name"], "New Agent");

    env.client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "What is the capital of France?"}))
        .send()
        .await
        .unwrap();

    // Name should now be set to the first message content.
    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["name"], "What is the capital of France?");
}

#[tokio::test]
async fn send_message_history_persisted_for_second_turn() {
    let env = start().await;

    // Both turns return end_turn.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("Paris"));
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    // Turn 1.
    let r1: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "What is the capital of France?"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let count_after_t1 = r1["message_count"].as_u64().unwrap();

    // Turn 2.
    let r2: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "And of Germany?"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let count_after_t2 = r2["message_count"].as_u64().unwrap();

    // History must have grown.
    assert!(
        count_after_t2 > count_after_t1,
        "second turn must append to history"
    );
    // At least 4 messages: user, assistant, user, assistant.
    assert!(count_after_t2 >= 4);
}

#[tokio::test]
async fn send_message_to_missing_session_returns_404() {
    let env = start().await;
    let res = env
        .client
        .post(format!("{}/sessions/ghost/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn send_message_missing_tenant_returns_400() {
    let env = start().await;
    let res = env
        .client
        .post(format!("{}/sessions/any/messages", env.base_url))
        .json(&json!({"content": "hi"}))
        .send()
        .await
        .unwrap();
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

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"model": "claude-haiku-4-5-20251001"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hello"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["content"], "haiku says hi");
    mock.assert_hits_async(1).await;
}

#[tokio::test]
async fn send_message_anthropic_500_returns_500() {
    let env = start().await;

    // Anthropic proxy always returns 500 — agent gets an HTTP error parsing JSON.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(500).body("Internal Server Error");
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let res = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hello"}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        500,
        "Anthropic 500 must propagate as HTTP 500"
    );
}

#[tokio::test]
async fn update_session_memory_path_persisted() {
    let env = start().await;

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "MP Session"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    // Initially no memory_path.
    assert!(created["memory_path"].is_null());

    // PATCH with memory_path.
    let updated: Value = env
        .client
        .patch(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"memory_path": "docs/memory.md"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["memory_path"], "docs/memory.md");

    // GET to confirm persistence.
    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"tools": ["get_pr_diff"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "show me the diff"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["content"], "diff result");
    filtered_mock.assert_hits_async(1).await;
    unfiltered_mock.assert_hits_async(0).await;
}

/// If a session is deleted while a `POST /sessions/:id/messages` request is
/// in flight, the message handler must complete successfully (returns 200) and
/// re-persist the session with the new messages via `session_store.put`.
///
/// The NATS KV `put` operation creates the key if absent, so the session is
/// effectively re-created after the message finishes.
#[tokio::test]
async fn send_message_completes_after_concurrent_session_delete() {
    let env = start().await;

    // Slow mock: Anthropic takes 400 ms to respond, giving us time to delete.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("reply after delete"))
            .delay(std::time::Duration::from_millis(400));
    });

    // Create the session.
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "race-test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let base_url = env.base_url.clone();
    let msg_client = env.client.clone();
    let msg_id = id.clone();

    // Launch the send_message request in a background task.
    let send_task = tokio::spawn(async move {
        msg_client
            .post(format!("{}/sessions/{msg_id}/messages", base_url))
            .header("x-tenant-id", "acme")
            .json(&json!({"content": "hello"}))
            .send()
            .await
            .unwrap()
    });

    // Delete the session while the message is still being processed.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let del = env
        .client
        .delete(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204, "delete must succeed");

    // Wait for the message request to complete.
    let resp = send_task.await.unwrap();
    assert_eq!(
        resp.status(),
        200,
        "send_message must return 200 even after concurrent delete"
    );

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["content"], "reply after delete");
}

/// Two concurrent `POST /sessions/:id/messages` requests to the same session
/// both return 200. The `send_message` handler persists with `session_store.put`
/// (no CAS), so the last write wins — one sender's assistant reply overwrites the
/// other's. This test documents that behaviour and asserts neither request errors.
#[tokio::test]
async fn concurrent_sends_to_same_session_both_succeed() {
    let env = start().await;

    // Slow Anthropic mock so both requests are in-flight at the same time.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("concurrent reply"))
            .delay(std::time::Duration::from_millis(200));
    });

    // Create a session.
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "concurrent-send-test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let c1 = env.client.clone();
    let c2 = env.client.clone();
    let url1 = format!("{}/sessions/{id}/messages", env.base_url);
    let url2 = url1.clone();

    // Launch both sends concurrently — both must return 200 regardless of
    // which write wins.
    let (r1, r2) = tokio::join!(
        async move {
            c1.post(url1)
                .header("x-tenant-id", "acme")
                .json(&json!({"content": "message A"}))
                .send()
                .await
                .unwrap()
        },
        async move {
            c2.post(url2)
                .header("x-tenant-id", "acme")
                .json(&json!({"content": "message B"}))
                .send()
                .await
                .unwrap()
        }
    );

    assert_eq!(r1.status(), 200, "first concurrent send must return 200");
    assert_eq!(r2.status(), 200, "second concurrent send must return 200");
}

#[tokio::test]
async fn get_session_missing_tenant_returns_400() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "tenant-check"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let res = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn patch_session_missing_tenant_returns_400() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "tenant-check-patch"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let res = env
        .client
        .patch(format!("{}/sessions/{id}", env.base_url))
        .json(&json!({"name": "new name"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn delete_session_missing_tenant_returns_400() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"name": "tenant-check-delete"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let res = env
        .client
        .delete(format!("{}/sessions/{id}", env.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

// ── Promise admin ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_promises_returns_empty_json_array() {
    let env = start().await;
    let res = env
        .client
        .get(format!("{}/admin/promises", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(
        body.is_empty(),
        "NoOpPromiseStore must return empty list; got {body:?}"
    );
}

#[tokio::test]
async fn list_promises_missing_tenant_returns_400() {
    let env = start().await;
    let res = env
        .client
        .get(format!("{}/admin/promises", env.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

// ── New-field behaviors ───────────────────────────────────────────────────────

#[tokio::test]
async fn create_session_sets_started_at_secs() {
    let env = start().await;
    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    // GET returns the full ChatSession which includes started_at_secs.
    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let started = got["started_at_secs"].as_u64().unwrap_or(0);
    assert!(
        started > 0,
        "started_at_secs must be set to current epoch on creation: {started}"
    );
}

#[tokio::test]
async fn send_message_computes_duration_ms() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("ok"));
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    // Sleep 1s so duration_ms = (now - started_at_secs) * 1000 >= 1000.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    env.client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "hello"}))
        .send()
        .await
        .unwrap();

    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let duration = got["duration_ms"].as_u64().unwrap_or(0);
    assert!(
        duration >= 1000,
        "duration_ms must be >= 1000 after a 1s sleep; got {duration}"
    );
}

#[tokio::test]
async fn create_session_stores_agent_id_from_state() {
    let env = start_with_options(Some("agent_test_001".to_string()), None, None).await;

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        got["agent_id"], "agent_test_001",
        "agent_id must be copied from ChatAppState into the session"
    );
}

#[tokio::test]
async fn send_message_persists_usage_on_assistant_message() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_with_usage("Paris", 42, 7));
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    env.client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "Capital of France?"}))
        .send()
        .await
        .unwrap();

    let got: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let messages = got["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message must be present");

    let usage = &assistant["usage"];
    assert_eq!(
        usage["input_tokens"], 42,
        "input_tokens from Anthropic response must be persisted"
    );
    assert_eq!(
        usage["output_tokens"], 7,
        "output_tokens from Anthropic response must be persisted"
    );
}

#[tokio::test]
async fn send_message_injects_skill_content_into_system_prompt() {
    let env = start_with_options(
        Some("agent_skills_test".to_string()),
        Some(Arc::new(FixedAgentLoader {
            target_id: "agent_skills_test".to_string(),
            skill_ids: vec!["skill_pdf".to_string()],
        })),
        Some(Arc::new(FixedSkillLoader {
            content: Some("# Available Skills\n\nThe following skills define specialized knowledge and procedures you must follow:\n\n## Skill: pdf-reader\n\nUse this to read PDFs.".to_string()),
        })),
    )
    .await;

    // Mock fires only when system prompt contains the skill header — must fire exactly once.
    let skill_mock = env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("Available Skills");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("skills received"));
    });

    let created: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let res: Value = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"content": "summarize the PDF"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["content"], "skills received");
    skill_mock.assert_hits_async(1).await;
}

// ── SSE helpers ───────────────────────────────────────────────────────────────

/// Parse incoming SSE chunks into JSON values and forward them on a channel.
///
/// Each SSE event is a `data: <json>\n\n` segment. The task runs until the
/// response body is exhausted or the receiver is dropped.
fn spawn_sse_reader(response: reqwest::Response) -> tokio::sync::mpsc::Receiver<Value> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(16);
    tokio::spawn(async move {
        let mut buf = String::new();
        let mut response = response;
        loop {
            if let Some(pos) = buf.find("\n\n") {
                let segment = buf[..pos].to_string();
                buf = buf[pos + 2..].to_string();
                for line in segment.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(val) = serde_json::from_str::<Value>(data) {
                            if tx.send(val).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                continue;
            }
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    let s = std::str::from_utf8(&chunk)
                        .unwrap_or("")
                        .replace("\r\n", "\n");
                    buf.push_str(&s);
                }
                _ => break,
            }
        }
    });
    rx
}

async fn create_session_id(env: &TestEnv) -> String {
    let res: Value = env
        .client
        .post(format!("{}/sessions", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    res["id"].as_str().unwrap().to_string()
}

// ── SSE approval / elicitation integration tests ──────────────────────────────

/// SSE mode with no tool calls: the stream must emit a single `done` event.
#[tokio::test]
async fn sse_plain_request_delivers_done_event() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("hello from sse"));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "hi"}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let mut rx = spawn_sse_reader(response);
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for SSE event")
        .expect("SSE channel closed before done event");

    assert_eq!(event["type"], "done");
    assert_eq!(event["content"], "hello from sse");
}

/// SSE mode with a tool call that gets approved: the stream must emit
/// `permission_required` followed by `done`.
#[tokio::test]
async fn sse_permission_approved_resumes_agent_to_done() {
    let env = start().await;

    // Register specific mock first — fires when the body contains "tool_use_id"
    // (i.e., the second turn where the agent returns the tool result).
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_use_id");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("tool approved and done"));
    });
    // General mock second — fires for the first turn (no tool result in body yet).
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tc-perm-1",
                    "name": "__test_perm_gate__",
                    "input": {}
                }]
            }));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "run the tool"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);

    // First event: permission gate fires before tool execution.
    let perm_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for permission_required event")
        .expect("SSE channel closed");

    assert_eq!(perm_event["type"], "permission_required");
    assert_eq!(perm_event["tool_call_id"], "tc-perm-1");
    assert_eq!(perm_event["tool_name"], "__test_perm_gate__");

    // Approve the tool call.
    let approve = env
        .client
        .post(format!("{}/sessions/{id}/approvals/tc-perm-1", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200);

    // Second event: agent completes after running the tool.
    let done_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for done event")
        .expect("SSE channel closed");

    assert_eq!(done_event["type"], "done");
    assert_eq!(done_event["content"], "tool approved and done");
}

/// SSE mode with a tool call that gets denied: the agent receives a
/// "Permission denied" tool result and still reaches `done`.
#[tokio::test]
async fn sse_permission_denied_agent_receives_denied_message() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_use_id");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("understood, tool denied"));
    });
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tc-deny-1",
                    "name": "__test_perm_gate__",
                    "input": {}
                }]
            }));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "run the tool"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);

    let perm_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for permission_required event")
        .expect("SSE channel closed");

    assert_eq!(perm_event["type"], "permission_required");
    assert_eq!(perm_event["tool_call_id"], "tc-deny-1");

    // Deny the tool call.
    let deny = env
        .client
        .post(format!("{}/sessions/{id}/approvals/tc-deny-1", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(deny.status(), 200);

    let done_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for done event")
        .expect("SSE channel closed");

    assert_eq!(done_event["type"], "done");
    assert_eq!(done_event["content"], "understood, tool denied");
}

/// SSE mode with `ask_user`: the stream emits `elicitation_required`, and
/// after the client posts an answer the agent completes with `done`.
#[tokio::test]
async fn sse_elicitation_answer_reaches_agent() {
    let env = start().await;

    // Second turn: agent received the user's answer as a tool result.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_use_id");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("got your answer"));
    });
    // First turn: model invokes the built-in ask_user tool.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tc-elicit-1",
                    "name": "ask_user",
                    "input": {"question": "What is your name?"}
                }]
            }));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "tell me"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);

    let elicit_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for elicitation_required event")
        .expect("SSE channel closed");

    assert_eq!(elicit_event["type"], "elicitation_required");
    assert_eq!(elicit_event["question"], "What is your name?");
    let elicitation_id = elicit_event["elicitation_id"].as_str().unwrap().to_string();

    // Post the user's answer.
    let answer = env
        .client
        .post(format!(
            "{}/sessions/{id}/elicitations/{elicitation_id}",
            env.base_url
        ))
        .header("x-tenant-id", "acme")
        .json(&json!({"answer": "Jorge"}))
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status(), 200);

    let done_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for done event")
        .expect("SSE channel closed");

    assert_eq!(done_event["type"], "done");
    assert_eq!(done_event["content"], "got your answer");
}

/// SSE mode with `ask_user` cancelled (null answer): the agent receives
/// "The user declined or cancelled the request." and still reaches `done`.
#[tokio::test]
async fn sse_elicitation_cancelled_delivers_done() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_use_id");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("understood, no answer"));
    });
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tc-cancel-1",
                    "name": "ask_user",
                    "input": {"question": "Confirm?"}
                }]
            }));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "confirm please"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);

    let elicit_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for elicitation_required event")
        .expect("SSE channel closed");

    assert_eq!(elicit_event["type"], "elicitation_required");
    let elicitation_id = elicit_event["elicitation_id"].as_str().unwrap().to_string();

    // Cancel (null answer).
    let cancel = env
        .client
        .post(format!(
            "{}/sessions/{id}/elicitations/{elicitation_id}",
            env.base_url
        ))
        .header("x-tenant-id", "acme")
        .json(&json!({"answer": null}))
        .send()
        .await
        .unwrap();
    assert_eq!(cancel.status(), 200);

    let done_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for done event")
        .expect("SSE channel closed");

    assert_eq!(done_event["type"], "done");
    assert_eq!(done_event["content"], "understood, no answer");
}

/// Posting to the approvals endpoint when no SSE stream is active returns 404.
#[tokio::test]
async fn approve_returns_404_when_no_active_sse_stream() {
    let env = start().await;

    let res = env
        .client
        .post(format!(
            "{}/sessions/nonexistent-session/approvals/some-tool-call",
            env.base_url
        ))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": true}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);
}

/// Posting to the elicitations endpoint when no SSE stream is active returns 404.
#[tokio::test]
async fn elicitation_respond_returns_404_when_no_active_sse_stream() {
    let env = start().await;

    let res = env
        .client
        .post(format!(
            "{}/sessions/nonexistent-session/elicitations/some-elicitation",
            env.base_url
        ))
        .header("x-tenant-id", "acme")
        .json(&json!({"answer": "hello"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);
}

/// The SSE response must carry `Content-Type: text/event-stream`.
#[tokio::test]
async fn sse_response_has_event_stream_content_type() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("ok"));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "hi"}))
        .send()
        .await
        .unwrap();

    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got: {ct}"
    );

    // Drain the stream.
    let mut rx = spawn_sse_reader(response);
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
}

/// The `done` event must carry the correct `message_count` (user + assistant = 2).
#[tokio::test]
async fn sse_done_event_message_count_is_correct() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("reply"));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "hello"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");

    assert_eq!(done["type"], "done");
    // One user message + one assistant message = 2.
    assert_eq!(done["message_count"], 2);
}

/// After the SSE stream closes, the session history must be persisted in NATS
/// so a subsequent GET reflects the completed turn.
#[tokio::test]
async fn sse_session_history_persisted_after_completion() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("stored reply"));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "persist me"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert_eq!(done["type"], "done");

    // session_store.put happens before the done event is sent, so the session
    // is already persisted by the time we receive it.
    let session: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let messages = session["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2, "user + assistant = 2 messages");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
}

/// When the Anthropic API errors during an SSE run the stream emits an `error`
/// event instead of `done`.
///
/// Uses 400 (non-retryable) so the agent fails immediately — 5xx would trigger
/// three retries with a 2 s + 4 s + 8 s backoff before giving up.
#[tokio::test]
async fn sse_anthropic_error_emits_error_event() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(400).body("Bad Request");
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "trigger error"}))
        .send()
        .await
        .unwrap();

    // SSE responds 200 immediately (stream starts before agent completes).
    assert_eq!(response.status(), 200);

    let mut rx = spawn_sse_reader(response);
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for error event")
        .expect("channel closed");

    assert_eq!(event["type"], "error");
    assert!(
        !event["message"].as_str().unwrap_or("").is_empty(),
        "error event must carry a non-empty message"
    );
}

/// Posting an approval for an unrecognised `tool_call_id` while the SSE stream
/// IS active (session registered) returns 404 — only the wrong key is missing.
#[tokio::test]
async fn sse_wrong_tool_call_id_returns_404_while_stream_active() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_use_id");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("done after corrected approval"));
    });
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tc-real-id",
                    "name": "__test_perm_gate__",
                    "input": {}
                }]
            }));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "run tool"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);

    // Wait until the agent is paused at the permission gate.
    let perm_event = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert_eq!(perm_event["type"], "permission_required");

    // Wrong ID → session IS registered but tool_call_id is not in the map.
    let wrong = env
        .client
        .post(format!("{}/sessions/{id}/approvals/wrong-tool-call-id", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 404);

    // Correct ID → unblocks the agent.
    let correct = env
        .client
        .post(format!("{}/sessions/{id}/approvals/tc-real-id", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(correct.status(), 200);

    let done = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert_eq!(done["type"], "done");
}

/// SSE mode also auto-names a session from the first user message.
#[tokio::test]
async fn sse_auto_names_session_from_first_message() {
    let env = start().await;

    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("ok"));
    });

    let id = create_session_id(&env).await;

    // Confirm initial name is the default.
    let before: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["name"], "New Agent");

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "rename me please"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");

    let after: Value = env
        .client
        .get(format!("{}/sessions/{id}", env.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["name"], "rename me please");
}

/// When the model returns two tool uses in a single response, the permission gate
/// fires sequentially — each must be approved before the agent proceeds.
#[tokio::test]
async fn sse_sequential_tool_approvals_both_required() {
    let env = start().await;

    // Second turn: both tool results are in the body.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_use_id");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn("both tools approved"));
    });
    // First turn: model returns two tool uses at once.
    env.mock_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "tc-seq-1",
                        "name": "__test_perm_gate__",
                        "input": {}
                    },
                    {
                        "type": "tool_use",
                        "id": "tc-seq-2",
                        "name": "__test_perm_gate__",
                        "input": {}
                    }
                ]
            }));
    });

    let id = create_session_id(&env).await;

    let response = env
        .client
        .post(format!("{}/sessions/{id}/messages", env.base_url))
        .header("x-tenant-id", "acme")
        .header("accept", "text/event-stream")
        .json(&json!({"content": "run two tools"}))
        .send()
        .await
        .unwrap();

    let mut rx = spawn_sse_reader(response);

    // Gate fires for first tool.
    let perm1 = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for first permission_required")
        .expect("channel closed");
    assert_eq!(perm1["type"], "permission_required");
    assert_eq!(perm1["tool_call_id"], "tc-seq-1");

    env.client
        .post(format!("{}/sessions/{id}/approvals/tc-seq-1", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": true}))
        .send()
        .await
        .unwrap();

    // Gate fires for second tool before the agent loops back to Anthropic.
    let perm2 = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for second permission_required")
        .expect("channel closed");
    assert_eq!(perm2["type"], "permission_required");
    assert_eq!(perm2["tool_call_id"], "tc-seq-2");

    env.client
        .post(format!("{}/sessions/{id}/approvals/tc-seq-2", env.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({"allowed": true}))
        .send()
        .await
        .unwrap();

    let done = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for done event")
        .expect("channel closed");
    assert_eq!(done["type"], "done");
    assert_eq!(done["content"], "both tools approved");
}
