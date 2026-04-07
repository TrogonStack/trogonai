//! Integration tests for [`AgentLoop`] using a mock HTTP server.
//!
//! These tests verify the full request-response cycle without hitting real
//! Anthropic, GitHub or Linear APIs.  A `httpmock::MockServer` stands in for
//! `trogon-secret-proxy`.

use std::sync::Arc;

use httpmock::MockServer;
use serde_json::json;
use trogon_agent::{
    agent_loop::{AgentError, AgentLoop, Message},
    tools::{ToolContext, tool_def},
};

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_agent(proxy_url: &str) -> AgentLoop {
    let http_client = reqwest::Client::new();
    AgentLoop {
        http_client: http_client.clone(),
        proxy_url: proxy_url.to_string(),
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 5,
        tool_context: Arc::new(ToolContext {
            http_client,
            proxy_url: proxy_url.to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        split_client: None,
        tenant_id: "test".to_string(),
    }
}

fn no_tools() -> Vec<trogon_agent::tools::ToolDef> {
    vec![]
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// The proxy returns `end_turn` immediately → agent returns the text content.
#[tokio::test]
async fn agent_loop_end_turn_returns_text() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .header("authorization", "Bearer tok_anthropic_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "LGTM — no issues found." }]
            }));
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(
            vec![Message::user_text("Review this PR")],
            &no_tools(),
            None,
        )
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "LGTM — no issues found.");
}

/// Multiple text blocks are joined with newlines.
#[tokio::test]
async fn agent_loop_end_turn_joins_multiple_text_blocks() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [
                    { "type": "text", "text": "Line one." },
                    { "type": "text", "text": "Line two." }
                ]
            }));
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("hello")], &no_tools(), None)
        .await
        .unwrap();
    assert_eq!(result, "Line one.\nLine two.");
}

/// `tool_use` on first response → agent calls the tool → sends result back →
/// second response is `end_turn`.
///
/// The two mocks are distinguished by request body:
/// - first request has no `tool_result` blocks → responds with tool_use
/// - second request carries the tool result → responds with end_turn
#[tokio::test]
async fn agent_loop_tool_use_then_end_turn() {
    let server = MockServer::start_async().await;

    // httpmock resolves mocks in FIFO order (first registered = first checked).
    // The specific mock (body_contains "tool_result") is registered first so it
    // wins on the second request; the fallback (no body constraint) wins on the
    // first request when the specific mock doesn't match.

    // Priority: POST with tool_result in body → end_turn (second request).
    let second = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "Done after tool." }]
            }));
    });

    // Fallback: any POST → tool_use (first request, no tool_result yet).
    let first = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tool_abc123",
                    "name": "unknown_tool_for_test",
                    "input": {}
                }]
            }));
    });

    let tools = vec![tool_def(
        "unknown_tool_for_test",
        "A test tool",
        json!({ "type": "object", "properties": {} }),
    )];

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("go")], &tools, None)
        .await
        .unwrap();

    assert_eq!(result, "Done after tool.");
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
}

/// When `max_iterations` is reached the agent returns `MaxIterationsReached`.
#[tokio::test]
async fn agent_loop_max_iterations_reached() {
    let server = MockServer::start_async().await;

    // Always respond with tool_use so the loop never ends naturally.
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "t1",
                    "name": "unknown_tool_for_test",
                    "input": {}
                }]
            }));
    });

    let http_client = reqwest::Client::new();
    let agent = AgentLoop {
        http_client: http_client.clone(),
        proxy_url: server.base_url(),
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 2, // low limit
        tool_context: Arc::new(ToolContext {
            http_client,
            proxy_url: server.base_url(),
            github_token: String::new(),
            linear_token: String::new(),
            slack_token: String::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        split_client: None,
        tenant_id: "test".to_string(),
    };

    let result = agent
        .run(vec![Message::user_text("loop")], &no_tools(), None)
        .await;
    assert!(matches!(result, Err(AgentError::MaxIterationsReached)));
}

/// An unexpected stop_reason returns `UnexpectedStopReason`.
#[tokio::test]
async fn agent_loop_unexpected_stop_reason() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "stop_sequence",
                "content": []
            }));
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("hi")], &no_tools(), None)
        .await;
    assert!(matches!(result, Err(AgentError::UnexpectedStopReason(r)) if r == "stop_sequence"));
}

/// An HTTP error (non-JSON body) causes `AgentError::Http`.
#[tokio::test]
async fn agent_loop_http_error_on_bad_response() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(500)
            .header("content-type", "text/plain")
            .body("internal server error");
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("hi")], &no_tools(), None)
        .await;
    assert!(matches!(result, Err(AgentError::Http(_))));
}

/// The Authorization header sent to the proxy carries the opaque token,
/// never a real API key.
#[tokio::test]
async fn agent_loop_sends_opaque_token_not_real_key() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .header("authorization", "Bearer tok_anthropic_prod_test01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "ok" }]
            }));
    });

    let agent = make_agent(&server.base_url());
    agent
        .run(vec![Message::user_text("hi")], &no_tools(), None)
        .await
        .unwrap();
    mock.assert_hits_async(1).await;
}

/// The `anthropic-version` header is always sent.
#[tokio::test]
async fn agent_loop_sends_anthropic_version_header() {
    let server = MockServer::start_async().await;

    let mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .header("anthropic-version", "2023-06-01");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "ok" }]
            }));
    });

    let agent = make_agent(&server.base_url());
    agent
        .run(vec![Message::user_text("hi")], &no_tools(), None)
        .await
        .unwrap();
    mock.assert_hits_async(1).await;
}

// ── run_chat ──────────────────────────────────────────────────────────────────

/// `run_chat` returns the final text AND the full updated message history.
#[tokio::test]
async fn run_chat_returns_text_and_history() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "The answer is 42." }]
            }));
    });

    let agent = make_agent(&server.base_url());
    let initial = vec![Message::user_text("What is the answer?")];
    let (text, history) = agent.run_chat(initial, &no_tools(), None).await.unwrap();

    assert_eq!(text, "The answer is 42.");
    // History: user message + assistant message.
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

/// `run_chat` extends pre-existing history with the new turn.
#[tokio::test]
async fn run_chat_extends_prior_history() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "Berlin." }]
            }));
    });

    // Simulate prior turn already in history.
    use trogon_agent::agent_loop::ContentBlock;
    let prior_history = vec![
        Message::user_text("What is the capital of France?"),
        Message::assistant(vec![ContentBlock::Text {
            text: "Paris.".to_string(),
        }]),
        Message::user_text("And Germany?"),
    ];

    let agent = make_agent(&server.base_url());
    let (text, history) = agent
        .run_chat(prior_history, &no_tools(), None)
        .await
        .unwrap();

    assert_eq!(text, "Berlin.");
    // 3 prior + 1 new assistant = 4.
    assert_eq!(history.len(), 4);
    assert_eq!(history[3].role, "assistant");
}

/// `run_chat` with a tool-use turn includes tool exchange in returned history.
#[tokio::test]
async fn run_chat_includes_tool_exchange_in_history() {
    let server = MockServer::start_async().await;

    // First response: tool_use.
    let second = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "Done." }]
            }));
    });

    let first = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tu_abc",
                    "name": "unknown_tool_for_test",
                    "input": {}
                }]
            }));
    });

    let tools = vec![trogon_agent::tools::tool_def(
        "unknown_tool_for_test",
        "A test tool",
        json!({ "type": "object", "properties": {} }),
    )];

    let agent = make_agent(&server.base_url());
    let (text, history) = agent
        .run_chat(vec![Message::user_text("use a tool")], &tools, None)
        .await
        .unwrap();

    assert_eq!(text, "Done.");
    // user + assistant(tool_use) + user(tool_result) + assistant(end_turn) = 4
    assert_eq!(history.len(), 4);
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
}

/// `run_chat` max_iterations returns `MaxIterationsReached` like `run`.
#[tokio::test]
async fn run_chat_max_iterations_reached() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "id": "t1", "name": "t", "input": {} }]
            }));
    });

    let http_client = reqwest::Client::new();
    let agent = AgentLoop {
        http_client: http_client.clone(),
        proxy_url: server.base_url(),
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 2,
        tool_context: Arc::new(ToolContext {
            http_client,
            proxy_url: server.base_url(),
            github_token: String::new(),
            linear_token: String::new(),
            slack_token: String::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        split_client: None,
        tenant_id: "test".to_string(),
    };

    let result = agent
        .run_chat(vec![Message::user_text("loop")], &no_tools(), None)
        .await;
    assert!(matches!(result, Err(AgentError::MaxIterationsReached)));
}
