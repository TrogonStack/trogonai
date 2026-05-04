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
