//! Integration tests for `AgentLoop` — uses a local httpmock server to simulate the Anthropic API.
//!
//! Run with:
//!   cargo test -p trogon-agent-core --test agent_loop_integration

use std::sync::Arc;

use httpmock::prelude::*;
use trogon_agent_core::agent_loop::{
    AgentError, AgentEvent, AgentLoop, ContentBlock, Message, PermissionChecker,
};
use trogon_agent_core::tools::{ToolContext, tool_def};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_agent(base_url: &str) -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http,
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        // Override the Anthropic endpoint so all requests hit our mock server.
        anthropic_base_url: Some(base_url.to_string()),
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    }
}

// ── JSON body helpers (used by run() and run_chat() tests) ────────────────────

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

fn unknown_stop_body() -> String {
    serde_json::json!({
        "stop_reason": "pause",
        "content": [{"type": "text", "text": "partial"}]
    })
    .to_string()
}

fn thinking_end_turn_body(thought: &str, text: &str) -> String {
    serde_json::json!({
        "stop_reason": "end_turn",
        "content": [
            {"type": "thinking", "thinking": thought},
            {"type": "text", "text": text}
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        }
    })
    .to_string()
}

#[allow(dead_code)]
fn max_tokens_with_thinking_body() -> String {
    serde_json::json!({
        "stop_reason": "max_tokens",
        "content": [
            {"type": "thinking", "thinking": "partial thoughts"},
            {"type": "text", "text": "partial answer"}
        ],
        "usage": {"input_tokens": 10, "output_tokens": 4096}
    })
    .to_string()
}

// ── SSE body helpers (used by run_chat_streaming() tests) ─────────────────────
//
// run_chat_streaming() always calls complete_streaming(), which expects an SSE
// response (text/event-stream). These helpers produce the correct SSE wire
// format so that SseParser can reconstruct stop_reason, content blocks, and
// usage from the chunks.

fn sse_event(event_type: &str, data: serde_json::Value) -> String {
    format!(
        "event: {event_type}\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap()
    )
}

fn sse_end_turn(text: &str) -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 5}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn sse_max_tokens() -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "partial response"}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "max_tokens"},
            "usage": {"output_tokens": 4096}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn sse_tool_use() -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "tu_001", "name": "unknown_tool"}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{}"}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 10}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn sse_unknown_stop() -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "partial"}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "pause"},
            "usage": {"output_tokens": 5}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn sse_thinking_end_turn(thought: &str, text: &str) -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "thinking", "thinking": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": thought}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 1,
            "content_block": {"type": "text", "text": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "text_delta", "text": text}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 1})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 5}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn sse_max_tokens_with_thinking() -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "thinking", "thinking": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "partial thoughts"}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 1,
            "content_block": {"type": "text", "text": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "text_delta", "text": "partial answer"}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 1})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "max_tokens"},
            "usage": {"output_tokens": 4096}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
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
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Streaming reply"));
    });

    let agent = make_agent(&server.base_url());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("stream me")], &[], None, tx, None)
        .await;

    assert!(result.is_ok(), "run_chat_streaming must succeed");

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

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
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Final text"));
    });

    let agent = make_agent(&server.base_url());
    let initial = vec![Message::user_text("tell me something")];
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let updated = agent
        .run_chat_streaming(initial, &[], None, tx, None)
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
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx, None)
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
            .header("Content-Type", "text/event-stream")
            .body(sse_max_tokens());
    });

    let agent = make_agent(&server.base_url());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx, None)
        .await;

    assert!(matches!(result, Err(AgentError::MaxTokens)));

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::UsageSummary { .. })),
        "expected UsageSummary event on max_tokens"
    );
}

// ── tool_use paths ────────────────────────────────────────────────────────────
//
// The trick: the second Anthropic call will contain "tool_result" in its body
// (the agent appends the tool result before retrying). Register the end_turn
// mock first with a body_contains filter so it only matches the second call;
// the catch-all tool_use mock is registered second and matches the first call.

/// `run()` processes a tool call and continues to `end_turn` on the next iteration.
/// Covers `execute_tools` and the `tool_use` branch of the main loop.
#[tokio::test]
async fn run_tool_use_then_end_turn() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done after tool"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("use a tool")], &[], None)
        .await;

    assert_eq!(result.unwrap(), "Done after tool");
}

/// `run_chat()` processes a tool call and appends it to the message history.
/// Covers the `tool_use` branch of `run_chat`.
#[tokio::test]
async fn run_chat_tool_use_then_end_turn() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Chat done after tool"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let agent = make_agent(&server.base_url());
    let (text, msgs) = agent
        .run_chat(vec![Message::user_text("hi")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "Chat done after tool");
    // History: user → assistant(tool_use) → user(tool_result) → assistant(text)
    assert!(
        msgs.len() >= 4,
        "expected at least 4 messages, got {}",
        msgs.len()
    );
}

/// `run_chat_streaming()` emits `ToolCallStarted` and `ToolCallFinished` events
/// when the model requests a tool call. Covers `execute_tools_streaming`.
#[tokio::test]
async fn run_chat_streaming_emits_tool_call_events() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Done after tool"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let agent = make_agent(&server.base_url());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("use a tool")], &[], None, tx, None)
        .await;

    assert!(result.is_ok(), "run_chat_streaming must succeed");

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "unknown_tool")
        ),
        "expected ToolCallStarted event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallFinished { .. })),
        "expected ToolCallFinished event"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::TextDelta { text } if text.contains("Done after tool"))
        ),
        "expected final TextDelta after tool"
    );
}

// ── UnexpectedStopReason ──────────────────────────────────────────────────────

/// `run()` returns `Err(UnexpectedStopReason)` for an unknown stop_reason.
/// Covers the `other =>` branch in the main loop.
#[tokio::test]
async fn run_unexpected_stop_reason() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(unknown_stop_body());
    });

    let agent = make_agent(&server.base_url());
    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;

    assert!(matches!(result, Err(AgentError::UnexpectedStopReason(_))));
}

/// `run_chat()` returns `Err(UnexpectedStopReason)` for an unknown stop_reason.
#[tokio::test]
async fn run_chat_unexpected_stop_reason() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(unknown_stop_body());
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run_chat(vec![Message::user_text("hi")], &[], None)
        .await;

    assert!(matches!(result, Err(AgentError::UnexpectedStopReason(_))));
}

/// `run_chat_streaming()` returns `Err(UnexpectedStopReason)` for an unknown stop_reason.
#[tokio::test]
async fn run_chat_streaming_unexpected_stop_reason() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_unknown_stop());
    });

    let agent = make_agent(&server.base_url());
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx, None)
        .await;

    assert!(matches!(result, Err(AgentError::UnexpectedStopReason(_))));
}

// ── MaxIterationsReached in run_chat / run_chat_streaming ─────────────────────

/// `run_chat()` returns `Err(MaxIterationsReached)` when always getting tool_use.
#[tokio::test]
async fn run_chat_max_iterations_reached() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let mut agent = make_agent(&server.base_url());
    agent.max_iterations = 2;

    let result = agent
        .run_chat(vec![Message::user_text("hi")], &[], None)
        .await;

    assert!(matches!(result, Err(AgentError::MaxIterationsReached)));
}

/// `run_chat_streaming()` returns `Err(MaxIterationsReached)` when always getting tool_use.
#[tokio::test]
async fn run_chat_streaming_max_iterations_reached() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let mut agent = make_agent(&server.base_url());
    agent.max_iterations = 2;

    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx, None)
        .await;

    assert!(matches!(result, Err(AgentError::MaxIterationsReached)));
}

// ── extra_headers / non-empty tools / system_prompt ──────────────────────────

/// `run()` forwards extra headers and marks the last tool with `cache_control`.
/// Covers: loop over `anthropic_extra_headers`, `cached_tools.last_mut()`.
#[tokio::test]
async fn run_with_extra_headers_and_tools() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("ok"));
    });

    let http = reqwest::Client::new();
    let tools = vec![tool_def("t", "d", serde_json::json!({"type": "object"}))];
    let agent = AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "tok".to_string(),
        anthropic_base_url: Some(server.base_url()),
        anthropic_extra_headers: vec![("X-Custom-Header".to_string(), "test-value".to_string())],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    };

    let result = agent
        .run(vec![Message::user_text("hi")], &tools, None)
        .await;
    assert_eq!(result.unwrap(), "ok");
}

/// `run_chat()` with system prompt, non-empty tools, and extra headers.
/// Covers: system block construction, cache_control marking, header loop.
#[tokio::test]
async fn run_chat_with_system_prompt_tools_and_extra_headers() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("chat ok"));
    });

    let http = reqwest::Client::new();
    let tools = vec![tool_def("t", "d", serde_json::json!({"type": "object"}))];
    let agent = AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "tok".to_string(),
        anthropic_base_url: Some(server.base_url()),
        anthropic_extra_headers: vec![("X-Custom-Header".to_string(), "test-value".to_string())],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    };

    let (text, msgs) = agent
        .run_chat(
            vec![Message::user_text("hi")],
            &tools,
            Some("You are helpful."),
        )
        .await
        .unwrap();
    assert_eq!(text, "chat ok");
    assert!(msgs.last().unwrap().role == "assistant");
}

// ── Thinking content blocks ───────────────────────────────────────────────────

/// `run()` ignores non-Text blocks (Thinking) when collecting the response text.
/// Covers the `else { None }` branch in the filter_map inside `end_turn`.
#[tokio::test]
async fn run_with_thinking_block_in_end_turn() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(thinking_end_turn_body("my thoughts", "final answer"));
    });

    let agent = make_agent(&server.base_url());
    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;

    assert_eq!(result.unwrap(), "final answer");
}

/// `run_chat()` ignores non-Text blocks when collecting the response text.
/// Covers the `else { None }` branch in the filter_map inside `end_turn` of `run_chat`.
#[tokio::test]
async fn run_chat_with_thinking_block_in_end_turn() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(thinking_end_turn_body("chain of thought", "chat answer"));
    });

    let agent = make_agent(&server.base_url());
    let (text, _msgs) = agent
        .run_chat(vec![Message::user_text("hi")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "chat answer");
}

// ── run_chat_streaming comprehensive coverage ─────────────────────────────────

/// `run_chat_streaming()` with thinking_budget, system_prompt, non-empty tools,
/// extra_headers, and a Thinking block in the response.
/// Covers: cache_control marking, system block construction, thinking_budget branch,
/// extra_headers loop, ThinkingDelta emission, and the None branch in filter_map.
#[tokio::test]
async fn run_chat_streaming_comprehensive() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_thinking_end_turn("internal reasoning", "streamed reply"));
    });

    let http = reqwest::Client::new();
    let tools = vec![tool_def("t", "d", serde_json::json!({"type": "object"}))];
    let agent = AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "tok".to_string(),
        anthropic_base_url: Some(server.base_url()),
        anthropic_extra_headers: vec![("X-Custom-Header".to_string(), "test-value".to_string())],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: Some(1000), // enables the thinking branch
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("think hard")],
            &tools,
            Some("You reason carefully."),
            tx,
            None,
        )
        .await;

    assert!(result.is_ok());

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::ThinkingDelta { text } if text.contains("internal reasoning"))
        ),
        "expected ThinkingDelta event"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::TextDelta { text } if text.contains("streamed reply"))
        ),
        "expected TextDelta event"
    );
}

/// `run_chat_streaming()` with a Thinking block in the max_tokens response.
/// Covers: the None branch in the filter_map inside the `max_tokens` handler.
#[tokio::test]
async fn run_chat_streaming_max_tokens_with_thinking_block() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_max_tokens_with_thinking());
    });

    let agent = make_agent(&server.base_url());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, tx, None)
        .await;

    assert!(matches!(result, Err(AgentError::MaxTokens)));

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::UsageSummary { .. })),
        "expected UsageSummary on max_tokens"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::TextDelta { text } if text.contains("partial answer"))
        ),
        "expected TextDelta with partial text"
    );
}

// ── permission_checker ────────────────────────────────────────────────────────

/// A `PermissionChecker` that always denies tool execution.
struct DenyAll;

impl PermissionChecker for DenyAll {
    fn check<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _tool_name: &'a str,
        _tool_input: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async { false })
    }
}

/// When a `permission_checker` denies the tool, `execute_tools_streaming` returns
/// a "Permission denied" message instead of executing the tool.
/// Covers the `Some(checker)` match arm and the `!allowed` branch.
#[tokio::test]
async fn run_chat_streaming_permission_denied() {
    let server = MockServer::start();
    // First call returns tool_use; second (with tool_result) returns end_turn.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let http = reqwest::Client::new();
    let agent = AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "tok".to_string(),
        anthropic_base_url: Some(server.base_url()),
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: Some(Arc::new(DenyAll)),
        elicitation_provider: None,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(vec![Message::user_text("use a tool")], &[], None, tx, None)
        .await;

    assert!(result.is_ok(), "should succeed after permission denial");

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

    // ToolCallFinished should carry the denial message
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCallFinished { output, .. } if output.contains("Permission denied")
        )),
        "expected ToolCallFinished with denial message"
    );
}

// ── proxy URL (else branch of messages_url) ───────────────────────────────────

/// Anthropic returns 200 OK but the body is not valid JSON.
/// The agent should return AgentError::Http (reqwest json parse error).
#[tokio::test]
async fn run_200_ok_with_invalid_json_body_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body("this is not json at all");
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("Say hello")], &[], None)
        .await;

    assert!(
        matches!(result, Err(AgentError::Http(_))),
        "200 OK with invalid JSON must return AgentError::Http, got: {:?}",
        result
    );
}

/// Anthropic returns 200 OK with valid JSON but missing required `stop_reason` field.
/// The agent should return AgentError::Http (serde deserialization error).
#[tokio::test]
async fn run_200_ok_with_missing_stop_reason_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"content": [{"type": "text", "text": "hello"}]}"#);
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("Say hello")], &[], None)
        .await;

    assert!(
        matches!(result, Err(AgentError::Http(_))),
        "200 OK missing stop_reason must return AgentError::Http, got: {:?}",
        result
    );
}

/// Anthropic returns 500 with a non-JSON error body.
/// The agent should return AgentError::Http.
#[tokio::test]
async fn run_500_with_plain_text_body_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(500)
            .header("Content-Type", "text/plain")
            .body("Internal Server Error");
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("Say hello")], &[], None)
        .await;

    assert!(
        matches!(result, Err(AgentError::Http(_))),
        "500 with plain text must return AgentError::Http, got: {:?}",
        result
    );
}

/// Anthropic returns 429 Too Many Requests.
#[tokio::test]
async fn run_429_rate_limit_returns_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(429)
            .header("Content-Type", "application/json")
            .body(r#"{"error": {"type": "rate_limit_error", "message": "Too many requests"}}"#);
    });

    let agent = make_agent(&server.base_url());
    let result = agent
        .run(vec![Message::user_text("Say hello")], &[], None)
        .await;

    assert!(
        matches!(result, Err(AgentError::Http(_))),
        "429 rate limit must return AgentError::Http, got: {:?}",
        result
    );
}

/// When `anthropic_base_url` is `None`, `messages_url()` builds the URL as
/// `{proxy_url}/anthropic/v1/messages`. Covers the else branch of `messages_url`.
#[tokio::test]
async fn run_uses_proxy_url_when_no_anthropic_base_url() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("via proxy"));
    });

    let http = reqwest::Client::new();
    let agent = AgentLoop {
        http_client: http.clone(),
        proxy_url: server.base_url(), // proxy_url points to mock
        anthropic_token: "tok".to_string(),
        anthropic_base_url: None, // <── use proxy path
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: "test".to_string(),
        max_iterations: 1,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    };

    let result = agent.run(vec![Message::user_text("hi")], &[], None).await;
    assert_eq!(result.unwrap(), "via proxy");
}

// ── steer ─────────────────────────────────────────────────────────────────────

/// A steer message pre-loaded into `steer_rx` is appended to the tool-results
/// turn and sent to the LLM in the second API request.
///
/// Verification trick: the second mock requires `body_contains("tool_result")`
/// AND `body_contains` of the steer text.  If the steer text is absent the
/// mock won't fire, the agent keeps looping on tool_use, and hits
/// MaxIterationsReached — making the test fail clearly.
#[tokio::test]
async fn run_chat_streaming_steer_appended_to_tool_results_turn() {
    let server = MockServer::start();
    // Second call: must contain both "tool_result" and the steer text.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("think step by step");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Done with steer"));
    });
    // First call: returns tool_use.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let agent = make_agent(&server.base_url());
    let (steer_tx, steer_rx) = tokio::sync::mpsc::channel(4);
    steer_tx.try_send("think step by step".to_string()).unwrap();

    let (event_tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("use a tool")],
            &[],
            None,
            event_tx,
            Some(steer_rx),
        )
        .await;

    assert!(
        result.is_ok(),
        "run_chat_streaming with steer must succeed: {result:?}"
    );
    assert_eq!(
        result.unwrap().last().unwrap().role,
        "assistant",
        "final message must be from assistant"
    );
}

/// The steer text appears in the returned message history as a `ContentBlock::Text`
/// block inside the tool-results user turn.
#[tokio::test]
async fn run_chat_streaming_steer_present_in_returned_messages() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let agent = make_agent(&server.base_url());
    let (steer_tx, steer_rx) = tokio::sync::mpsc::channel(4);
    steer_tx.try_send("steer guidance".to_string()).unwrap();

    let (event_tx, _rx) = tokio::sync::mpsc::channel(32);
    let messages = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, event_tx, Some(steer_rx))
        .await
        .unwrap();

    // Find the tool-results user turn (role == "user" containing a ToolResult block).
    let tool_results_turn = messages.iter().find(|m| {
        m.role == "user"
            && m.content
                .iter()
                .any(|c| matches!(c, ContentBlock::ToolResult { .. }))
    });
    assert!(
        tool_results_turn.is_some(),
        "expected a tool-results user turn in message history"
    );
    let has_steer = tool_results_turn.unwrap().content.iter().any(|c| {
        matches!(c, ContentBlock::Text { text } if text == "steer guidance")
    });
    assert!(
        has_steer,
        "steer text must appear as a Text block in the tool-results turn"
    );
}

/// Multiple steer messages are ALL appended to the tool-results turn.
#[tokio::test]
async fn run_chat_streaming_multiple_steer_messages_all_appended() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("hint one")
            .body_contains("hint two")
            .body_contains("hint three");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Done with all hints"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let agent = make_agent(&server.base_url());
    let (steer_tx, steer_rx) = tokio::sync::mpsc::channel(8);
    steer_tx.try_send("hint one".to_string()).unwrap();
    steer_tx.try_send("hint two".to_string()).unwrap();
    steer_tx.try_send("hint three".to_string()).unwrap();

    let (event_tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("use a tool")],
            &[],
            None,
            event_tx,
            Some(steer_rx),
        )
        .await;

    assert!(
        result.is_ok(),
        "all three steer messages must reach the LLM: {result:?}"
    );
}

/// When the model returns `end_turn` directly (no tool call), steer messages
/// in `steer_rx` are silently dropped — the prompt still succeeds.
#[tokio::test]
async fn run_chat_streaming_steer_ignored_without_tool_use() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Direct reply"));
    });

    let agent = make_agent(&server.base_url());
    let (steer_tx, steer_rx) = tokio::sync::mpsc::channel(4);
    steer_tx.try_send("ignored steer".to_string()).unwrap();

    let (event_tx, _rx) = tokio::sync::mpsc::channel(32);
    let messages = agent
        .run_chat_streaming(vec![Message::user_text("hi")], &[], None, event_tx, Some(steer_rx))
        .await
        .unwrap();

    // The steer text must not appear anywhere in the returned message history.
    let steer_found = messages.iter().any(|m| {
        m.content.iter().any(|c| {
            matches!(c, ContentBlock::Text { text } if text.contains("ignored steer"))
        })
    });
    assert!(
        !steer_found,
        "steer text must not appear in messages when there was no tool call"
    );
    assert_eq!(
        messages.last().unwrap().role,
        "assistant",
        "last message must still be from assistant"
    );
}

/// Passing `steer_rx = None` leaves existing behaviour unchanged.
#[tokio::test]
async fn run_chat_streaming_steer_none_is_noop() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Done no steer"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use());
    });

    let agent = make_agent(&server.base_url());
    let (event_tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("use a tool")],
            &[],
            None,
            event_tx,
            None, // no steer
        )
        .await;

    assert!(
        result.is_ok(),
        "tool use without steer must still succeed: {result:?}"
    );
}

// ── PR 1: real tool dispatch integration ─────────────────────────────────────

/// When the model returns two `tool_use` blocks in a single response, both
/// are dispatched and both `tool_result`s appear in the follow-up request.
#[tokio::test]
async fn run_chat_multiple_tool_use_blocks_all_results_sent_back() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "content_a").await.unwrap();
    tokio::fs::write(dir.path().join("b.txt"), "content_b").await.unwrap();

    let server = MockServer::start();

    // Second call: both tool results must be present.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("content_a")
            .body_contains("content_b");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("both files read"));
    });

    // First call: two tool_use blocks in one response.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [
                        {"type": "tool_use", "id": "tu_a", "name": "read_file", "input": {"path": "a.txt"}},
                        {"type": "tool_use", "id": "tu_b", "name": "read_file", "input": {"path": "b.txt"}}
                    ]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, msgs) = agent
        .run_chat(vec![Message::user_text("read both files")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "both files read");
    // The tool_results message must contain two ToolResult blocks.
    let tool_result_msg = msgs.iter().find(|m| {
        m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
    });
    assert!(tool_result_msg.is_some(), "expected a tool_results message");
    let result_count = tool_result_msg.unwrap().content.iter()
        .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
        .count();
    assert_eq!(result_count, 2, "expected 2 tool results, got {result_count}");
}

/// When a tool fails (file does not exist), the error string is sent back as
/// `tool_result` and the agent loop continues without panicking.
#[tokio::test]
async fn run_chat_tool_error_propagated_as_tool_result() {
    let server = MockServer::start();

    // Second call: the tool_result must contain an error string, not a crash.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("handled error"));
    });

    // First call: tool_use for read_file on a file that does not exist.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_err",
                        "name": "read_file",
                        "input": {"path": "does_not_exist.txt"}
                    }]
                })
                .to_string(),
            );
    });

    // Use a tempdir as cwd so the file is guaranteed to not exist.
    let dir = tempfile::TempDir::new().unwrap();
    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let result = agent
        .run_chat(vec![Message::user_text("read missing file")], &[], None)
        .await;

    // The agent must not crash — it should complete and return the final text.
    assert_eq!(result.unwrap().0, "handled error");
}

/// The agent receives a `tool_use` block for `read_file`, dispatches it to the
/// real `fs::read_file` implementation, and sends the file content back as a
/// `tool_result` in the next request.
#[tokio::test]
async fn run_chat_real_read_file_tool_result_sent_back() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("hello.txt"), "integration content")
        .await
        .unwrap();

    let server = MockServer::start();

    // Second call: must contain both "tool_result" and the file's actual content.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("integration content");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("file read successfully"));
    });

    // First call: return a tool_use for read_file targeting the temp file.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_rf_001",
                        "name": "read_file",
                        "input": {"path": "hello.txt"}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _msgs) = agent
        .run_chat(vec![Message::user_text("read the file")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "file read successfully");
}

/// `ToolContext.cwd` is used when resolving relative paths in real tool
/// execution: a file only reachable via `cwd` is found and its content is
/// included in the tool_result sent to the API.
#[tokio::test]
async fn run_chat_cwd_is_propagated_to_tool_execution() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("marker.txt"), "cwd_marker_value")
        .await
        .unwrap();

    let server = MockServer::start();

    // Second call: the tool_result must contain the content from the cwd file.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("cwd_marker_value");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("cwd confirmed"));
    });

    // First call: tool_use for read_file with a relative path.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_cwd_001",
                        "name": "read_file",
                        "input": {"path": "marker.txt"}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let result = agent
        .run_chat(vec![Message::user_text("check cwd")], &[], None)
        .await;

    assert_eq!(result.unwrap().0, "cwd confirmed");
}

/// When `all_tool_defs()` is passed to `run_chat`, the request body sent to
/// the Anthropic API contains the tool schemas (verified by name).
#[tokio::test]
async fn run_chat_all_tool_defs_present_in_request_body() {
    use trogon_agent_core::tools::all_tool_defs;

    let server = MockServer::start();

    // Verify the request body contains several tool names from all_tool_defs().
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("read_file")
            .body_contains("write_file")
            .body_contains("str_replace")
            .body_contains("fetch_url")
            .body_contains("git_status");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("tools received"));
    });

    let agent = make_agent(&server.base_url());
    let (text, _) = agent
        .run_chat(
            vec![Message::user_text("hi")],
            &all_tool_defs(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(text, "tools received");
}

/// The agent receives a `tool_use` block for `write_file`, dispatches it to the
/// real `fs::write_file` implementation, and the file is created on disk.
/// The tool_result "OK" is sent back in the next request.
#[tokio::test]
async fn run_chat_real_write_file_creates_file_on_disk() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("output.txt");

    let server = MockServer::start();

    // Second call: tool_result must contain "OK".
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("OK");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("file written"));
    });

    // First call: tool_use for write_file.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_wf_001",
                        "name": "write_file",
                        "input": {"path": "output.txt", "content": "written_by_agent"}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("write a file")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "file written");
    let contents = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(contents, "written_by_agent");
}

/// The agent receives a `tool_use` block for `str_replace`, dispatches it to
/// the real `editor::str_replace` implementation, and the file is modified on disk.
/// The diff is included in the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_str_replace_modifies_file_on_disk() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("src.rs"), "fn old_name() {}\n")
        .await
        .unwrap();

    let server = MockServer::start();

    // Second call: tool_result must contain the diff (shows the replacement).
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("new_name");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("file edited"));
    });

    // First call: tool_use for str_replace.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_sr_001",
                        "name": "str_replace",
                        "input": {
                            "path": "src.rs",
                            "old_str": "old_name",
                            "new_str": "new_name"
                        }
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("rename the function")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "file edited");
    let contents = tokio::fs::read_to_string(dir.path().join("src.rs"))
        .await
        .unwrap();
    assert!(contents.contains("new_name"), "got: {contents}");
    assert!(!contents.contains("old_name"), "old name still present: {contents}");
}

/// The agent receives a `tool_use` block for `git_status`, dispatches it to
/// the real `git::status` implementation, and the real git output is included
/// in the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_git_status_result_sent_back() {
    use std::process::Command;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();

    // Initialise a real git repo with one untracked file.
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    tokio::fs::write(dir.path().join("untracked.rs"), "")
        .await
        .unwrap();

    let server = MockServer::start();

    // Second call: tool_result must contain the git status output.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("untracked.rs");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("git status received"));
    });

    // First call: tool_use for git_status.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_gs_001",
                        "name": "git_status",
                        "input": {}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("check git status")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "git status received");
}

/// The agent receives a `tool_use` block for `list_dir`, dispatches it to
/// the real `fs::list_dir` implementation, and filenames are included in
/// the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_list_dir_result_sent_back() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("alpha.rs"), "").await.unwrap();
    tokio::fs::write(dir.path().join("beta.rs"), "").await.unwrap();

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("alpha.rs");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("list dir done"));
    });

    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_ld_001",
                        "name": "list_dir",
                        "input": {}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("list the directory")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "list dir done");
}

/// The agent receives a `tool_use` block for `glob`, dispatches it to the
/// real `fs::glob_files` implementation, and matching filenames are included
/// in the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_glob_result_sent_back() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("main.rs"), "").await.unwrap();
    tokio::fs::write(dir.path().join("lib.rs"), "").await.unwrap();
    tokio::fs::write(dir.path().join("config.toml"), "").await.unwrap();

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("main.rs");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("glob done"));
    });

    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_gb_001",
                        "name": "glob",
                        "input": { "pattern": "*.rs" }
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("find rust files")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "glob done");
}

/// The agent receives a `tool_use` block for `git_diff`, dispatches it to
/// the real `git::diff` implementation, and the diff output is included in
/// the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_git_diff_result_sent_back() {
    use std::process::Command;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();

    Command::new("git").args(["init"]).current_dir(dir.path()).output().unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    tokio::fs::write(dir.path().join("file.rs"), "fn original() {}\n").await.unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    tokio::fs::write(dir.path().join("file.rs"), "fn modified() {}\n").await.unwrap();

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("modified");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("git diff done"));
    });

    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_gd_001",
                        "name": "git_diff",
                        "input": {}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("show the diff")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "git diff done");
}

/// The agent receives a `tool_use` block for `git_log`, dispatches it to
/// the real `git::log` implementation, and the log output (commit message)
/// is included in the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_git_log_result_sent_back() {
    use std::process::Command;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();

    Command::new("git").args(["init"]).current_dir(dir.path()).output().unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    tokio::fs::write(dir.path().join("readme.txt"), "hello").await.unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial-commit-marker"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("initial-commit-marker");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("git log done"));
    });

    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_gl_001",
                        "name": "git_log",
                        "input": {}
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("show the log")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "git log done");
}

/// The agent receives a `tool_use` block for `notebook_edit`, dispatches it
/// to the real `fs::notebook_edit` implementation, the cell is updated on
/// disk, and "OK" is included in the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_notebook_edit_modifies_cell() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let notebook = serde_json::json!({
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {},
        "cells": [{
            "cell_type": "code",
            "source": ["old_content"],
            "metadata": {},
            "outputs": [],
            "execution_count": null
        }]
    });
    tokio::fs::write(
        dir.path().join("nb.ipynb"),
        serde_json::to_string(&notebook).unwrap(),
    )
    .await
    .unwrap();

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("OK");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("notebook edited"));
    });

    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_nb_001",
                        "name": "notebook_edit",
                        "input": {
                            "path": "nb.ipynb",
                            "cell_index": 0,
                            "content": "new_content"
                        }
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("edit the notebook")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "notebook edited");
    let raw = tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap();
    assert!(raw.contains("new_content"), "cell not updated on disk: {raw}");
}

/// The agent receives a `tool_use` block for `fetch_url`, dispatches it to
/// the real `web::fetch_url` implementation, and the response body is
/// included in the tool_result sent back to the API.
#[tokio::test]
async fn run_chat_real_fetch_url_response_content_sent_back() {
    use httpmock::prelude::*;

    let file_server = MockServer::start();
    file_server.mock(|when, then| {
        when.method(GET).path("/data.txt");
        then.status(200)
            .header("Content-Type", "text/plain")
            .body("unique-payload-xyz");
    });

    let api_server = MockServer::start();

    api_server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("unique-payload-xyz");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("fetch url done"));
    });

    let fetch_url = file_server.url("/data.txt");
    api_server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_fu_001",
                        "name": "fetch_url",
                        "input": { "url": fetch_url, "raw": true }
                    }]
                })
                .to_string(),
            );
    });

    let mut agent = make_agent(&api_server.base_url());
    agent.tool_context = Arc::new(ToolContext {
        proxy_url: "http://127.0.0.1:1".to_string(),
        cwd: ".".to_string(),
        http_client: reqwest::Client::new(),
    });

    let (text, _) = agent
        .run_chat(vec![Message::user_text("fetch the url")], &[], None)
        .await
        .unwrap();

    assert_eq!(text, "fetch url done");
}
