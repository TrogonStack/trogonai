//! Integration tests for `AgentLoop` — uses a local httpmock server to simulate the Anthropic API.
//!
//! Run with:
//!   cargo test -p trogon-agent-core --test agent_loop_integration

use std::sync::Arc;

use httpmock::prelude::*;
use trogon_agent_core::agent_loop::{AgentError, AgentEvent, AgentLoop, Message};
use trogon_agent_core::tools::ToolContext;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_agent(base_url: &str) -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        // Override the Anthropic endpoint so all requests hit our mock server.
        anthropic_base_url: Some(base_url.to_string()),
        anthropic_extra_headers: vec![],
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            http_client: http,
            proxy_url: "http://127.0.0.1:1".to_string(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        split_client: None,
        tenant_id: "test-tenant".to_string(),
        permission_checker: None,
    }
}

fn end_turn_body(text: &str) -> String {
    serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": text}],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        }
    })
    .to_string()
}

fn max_tokens_body() -> String {
    serde_json::json!({
        "stop_reason": "max_tokens",
        "content": [{"type": "text", "text": "partial response"}],
        "usage": {"input_tokens": 10, "output_tokens": 4096}
    })
    .to_string()
}

fn tool_use_body() -> String {
    serde_json::json!({
        "stop_reason": "tool_use",
        "content": [{"type": "tool_use", "id": "tu_001", "name": "unknown_tool", "input": {}}]
    })
    .to_string()
}

// ── AgentLoop::run ────────────────────────────────────────────────────────────

/// Happy path: model returns `end_turn` with a text block → `run()` returns the text.
#[tokio::test]
async fn run_end_turn_returns_text() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Hello, World!"));
    });

    let agent = make_agent(&server.base_url());
    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;

    assert_eq!(result.unwrap(), "Hello, World!");
}

/// When the model returns `max_tokens`, `run()` returns `Err(MaxTokens)`.
#[tokio::test]
async fn run_max_tokens_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(max_tokens_body());
    });

    let agent = make_agent(&server.base_url());
    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;

    assert!(matches!(result, Err(AgentError::MaxTokens)));
}

/// When the model always returns `tool_use` and `max_iterations` is exhausted,
/// `run()` returns `Err(MaxIterationsReached)`.
#[tokio::test]
async fn run_max_iterations_reached_when_always_tool_use() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let mut agent = make_agent(&server.base_url());
    agent.max_iterations = 2; // 2 iterations, each returns tool_use → MaxIterationsReached

    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;

    assert!(matches!(result, Err(AgentError::MaxIterationsReached)));
}

/// When the Anthropic endpoint is unreachable, `run()` returns `Err(Http(_))`.
#[tokio::test]
async fn run_http_error_returns_error() {
    // Nothing listens at port 1 — guaranteed connection refused.
    let agent = make_agent("http://127.0.0.1:1");
    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;

    assert!(matches!(result, Err(AgentError::Http(_))));
}

/// With a system prompt, the model still responds normally.
#[tokio::test]
async fn run_with_system_prompt_succeeds() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Got it."));
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(
            vec![Message::user_text("follow the rules")],
            &[],
            Some("You are a helpful assistant."),
        )
        .await;

    assert_eq!(result.unwrap(), "Got it.");
}

// ── AgentLoop::run_chat ───────────────────────────────────────────────────────

/// `run_chat()` returns the model's text and the updated message history.
/// The history must contain at least the original user message and the assistant reply.
#[tokio::test]
async fn run_chat_returns_text_and_updated_messages() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Chat reply"));
    });

    let agent = make_agent(&server.base_url());
    let initial = vec![Message::user_text("what is 2+2?")];
    let (text, updated) = agent.run_chat(initial, &[], None).await.unwrap();

    assert_eq!(text, "Chat reply");
    assert!(
        updated.len() >= 2,
        "expected at least user + assistant in history"
    );
    assert_eq!(updated.last().unwrap().role, "assistant");
}

/// `run_chat()` preserves prior turns: the returned history starts with the
/// initial messages and ends with the new assistant reply.
#[tokio::test]
async fn run_chat_history_grows_with_each_turn() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Turn 1 reply"));
    });

    let agent = make_agent(&server.base_url());
    let initial = vec![Message::user_text("first message")];
    let (_, history) = agent.run_chat(initial.clone(), &[], None).await.unwrap();

    // History includes the initial user message plus the assistant reply.
    assert!(history.len() >= 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history.last().unwrap().role, "assistant");
}

/// When `max_tokens` is returned, `run_chat()` propagates `Err(MaxTokens)`.
#[tokio::test]
async fn run_chat_max_tokens_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(max_tokens_body());
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run_chat(vec![Message::user_text("hi")], &[], None)
        .await;

    assert!(matches!(result, Err(AgentError::MaxTokens)));
}

// ── AgentLoop::run_chat_streaming ─────────────────────────────────────────────

/// `run_chat_streaming()` emits `TextDelta` and `UsageSummary` events on `end_turn`.
#[tokio::test]
async fn run_chat_streaming_emits_text_delta_and_usage_summary() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Streaming reply"));
    });

    let agent = make_agent(&server.base_url());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("stream me")], &[], None, tx)
        .await;

    assert!(result.is_ok(), "run_chat_streaming must succeed");

    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "Streaming reply")),
        "expected TextDelta event with correct text"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::UsageSummary { .. })),
        "expected UsageSummary event"
    );
}

/// On `end_turn`, the returned message history includes the assistant reply.
#[tokio::test]
async fn run_chat_streaming_returns_updated_history() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Final text"));
    });

    let agent = make_agent(&server.base_url());
    let initial = vec![Message::user_text("tell me something")];
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let updated = agent
        .run_chat_streaming(initial, &[], None, tx)
        .await
        .unwrap();

    assert!(updated.len() >= 2);
    assert_eq!(updated.last().unwrap().role, "assistant");
}

/// When the endpoint is unreachable, `run_chat_streaming()` returns `Err(Http(_))`.
#[tokio::test]
async fn run_chat_streaming_http_error_returns_error() {
    let agent = make_agent("http://127.0.0.1:1");
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx)
        .await;

    assert!(matches!(result, Err(AgentError::Http(_))));
}

/// On `max_tokens`, `run_chat_streaming()` emits `UsageSummary` (and optionally
/// `TextDelta` if there was partial text) then returns `Err(MaxTokens)`.
#[tokio::test]
async fn run_chat_streaming_max_tokens_emits_usage_and_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(max_tokens_body());
    });

    let agent = make_agent(&server.base_url());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx)
        .await;

    assert!(matches!(result, Err(AgentError::MaxTokens)));

    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::UsageSummary { .. })),
        "expected UsageSummary event on max_tokens"
    );
}
