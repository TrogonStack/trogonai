//! Integration tests for `Runner` — requires Docker (testcontainers starts NATS).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test runner_e2e

use std::sync::Arc;
use std::time::Duration;

use acp_nats::nats::agent as subjects;
use acp_nats::prompt_event::{PromptEvent, PromptPayload, UserContentBlock};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use httpmock::prelude::*;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::RwLock;
use trogon_acp_runner::Runner;
use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
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

fn make_agent(base_url: &str) -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
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
        permission_checker: None,
    }
}

fn tool_use_body() -> String {
    serde_json::json!({
        "stop_reason": "tool_use",
        "content": [{"type": "tool_use", "id": "tu_001", "name": "unknown_tool", "input": {}}]
    })
    .to_string()
}

fn max_tokens_body() -> String {
    serde_json::json!({
        "stop_reason": "max_tokens",
        "content": [{"type": "text", "text": "partial"}],
        "usage": {"input_tokens": 10, "output_tokens": 4096}
    })
    .to_string()
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

/// Collect events from `sub` until a `Done` or `Error` event arrives (or timeout).
/// Returns all events received.
async fn collect_until_done(
    sub: &mut async_nats::Subscriber,
    timeout_secs: u64,
) -> Vec<PromptEvent> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut events = vec![];
    loop {
        let msg = tokio::time::timeout_at(deadline, sub.next())
            .await
            .expect("timed out waiting for prompt event")
            .expect("events subscription ended unexpectedly");
        let event: PromptEvent =
            serde_json::from_slice(&msg.payload).expect("invalid PromptEvent JSON");
        let is_terminal = matches!(event, PromptEvent::Done { .. } | PromptEvent::Error { .. });
        events.push(event);
        if is_terminal {
            break;
        }
    }
    events
}

// ── Runner::new ───────────────────────────────────────────────────────────────

/// `Runner::new` succeeds and creates the `ACP_SESSIONS` KV bucket.
/// After creation, `SessionStore::open` on the same JetStream context is idempotent.
#[tokio::test]
async fn runner_new_creates_session_bucket() {
    let (_c, nats, js) = start_nats().await;
    let agent = make_agent("http://127.0.0.1:1");

    let runner = Runner::new(
        nats,
        &js,
        agent,
        "test-new",
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .expect("Runner::new must succeed");

    // Opening the store again must be idempotent (bucket already exists).
    trogon_acp_runner::SessionStore::open(&js)
        .await
        .expect("SessionStore::open must succeed after Runner::new");

    drop(runner);
}

// ── Runner::run — error path ──────────────────────────────────────────────────

/// When the Anthropic endpoint is unreachable, the runner publishes an `Error`
/// or `Done` event (connection-refused is an HTTP error → `AgentError::Http`).
#[tokio::test]
async fn runner_publishes_error_event_when_anthropic_unreachable() {
    let (_c, nats, js) = start_nats().await;

    // Port 1 = connection refused, so reqwest will fail immediately.
    let agent = make_agent("http://127.0.0.1:1");
    let prefix = "test-err";
    let session_id = "sess-err-1";
    let req_id = "req-err-1";

    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats
        .subscribe(events_subject)
        .await
        .expect("subscribe to events");

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    // Runner uses spawn_local internally → must run inside a LocalSet.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });

            // Give the runner time to subscribe to its wildcard subject.
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Publish a prompt.
            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "hello".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            // Collect events until we get Error or Done.
            let events = collect_until_done(&mut events_sub, 10).await;

            // At least one terminal event must have arrived.
            let terminal = events
                .iter()
                .find(|e| matches!(e, PromptEvent::Error { .. } | PromptEvent::Done { .. }));
            assert!(
                terminal.is_some(),
                "expected Error or Done event, got: {events:?}"
            );
        })
        .await;
}

// ── Runner::run — happy path ──────────────────────────────────────────────────

/// When the Anthropic API returns `end_turn`, the runner publishes `TextDelta`
/// then `Done { stop_reason: "end_turn" }`.
#[tokio::test]
async fn runner_publishes_done_end_turn_with_mock_anthropic() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Great response!"));
    });

    let prefix = "test-done";
    let session_id = "sess-done-1";
    let req_id = "req-done-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats
        .subscribe(events_subject)
        .await
        .expect("subscribe to events");

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "hello".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            let events = collect_until_done(&mut events_sub, 15).await;

            let text_delta = events
                .iter()
                .find(|e| matches!(e, PromptEvent::TextDelta { text } if text.contains("Great response!")));
            assert!(text_delta.is_some(), "expected TextDelta event");

            let done = events.iter().find(|e| {
                matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn")
            });
            assert!(done.is_some(), "expected Done(end_turn) event");
        })
        .await;
}

/// Session state is persisted after a successful turn — a second prompt resumes
/// the conversation (the history grows).
#[tokio::test]
async fn runner_persists_session_after_end_turn() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Saved reply"));
    });

    let prefix = "test-persist";
    let session_id = "sess-persist-1";
    let req_id = "req-persist-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "persist me".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            collect_until_done(&mut events_sub, 15).await;

            // After the turn, the session must be persisted in KV.
            let store = trogon_acp_runner::SessionStore::open(&js).await.unwrap();
            let state = store.load(session_id).await.unwrap();

            // History must contain the user message + assistant reply (>= 2 messages).
            assert!(
                state.messages.len() >= 2,
                "expected at least 2 messages in persisted session, got {}",
                state.messages.len()
            );
            // Title is captured from the first prompt.
            assert!(!state.title.is_empty(), "expected non-empty session title");
        })
        .await;
}

/// A malformed prompt payload (invalid JSON) is silently skipped — the runner
/// keeps listening and processes the next valid prompt without crashing.
#[tokio::test]
async fn runner_skips_invalid_prompt_payload() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("After skip"));
    });

    let prefix = "test-skip";
    let session_id = "sess-skip-1";
    let req_id = "req-skip-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Send garbage first — runner must skip it without crashing.
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(b"not valid json".to_vec()),
            )
            .await
            .unwrap();

            // Give runner a moment to process (and discard) the bad message.
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Now send a valid prompt — runner must still handle it.
            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "valid".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            let events = collect_until_done(&mut events_sub, 15).await;

            let done = events
                .iter()
                .find(|e| matches!(e, PromptEvent::Done { .. }));
            assert!(
                done.is_some(),
                "expected Done event after skipping bad payload"
            );
        })
        .await;
}

// ── Runner::run — error stop reasons ─────────────────────────────────────────

/// When Anthropic returns `max_tokens`, the runner publishes `Done { stop_reason: "max_tokens" }`.
#[tokio::test]
async fn runner_publishes_done_max_tokens() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(max_tokens_body());
    });

    let prefix = "test-maxtok";
    let session_id = "sess-maxtok-1";
    let req_id = "req-maxtok-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "fill the context".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            let events = collect_until_done(&mut events_sub, 15).await;

            let done = events.iter().find(
                |e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "max_tokens"),
            );
            assert!(done.is_some(), "expected Done(max_tokens) event");
        })
        .await;
}

/// When `max_iterations` is exhausted (model always returns `tool_use`),
/// the runner publishes `Done { stop_reason: "max_turn_requests" }`.
#[tokio::test]
async fn runner_publishes_done_max_turn_requests() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let prefix = "test-maxiter";
    let session_id = "sess-maxiter-1";
    let req_id = "req-maxiter-1";

    let mut agent = make_agent(&server.base_url());
    agent.max_iterations = 1; // exhaust after one tool_use → MaxIterationsReached

    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "loop forever".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            let events = collect_until_done(&mut events_sub, 15).await;

            let done = events.iter().find(|e| {
                matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "max_turn_requests")
            });
            assert!(done.is_some(), "expected Done(max_turn_requests) event");
        })
        .await;
}

/// When the model requests a tool call, the runner publishes `ToolCallStarted`
/// and `ToolCallFinished` events before the final `Done`.
#[tokio::test]
async fn runner_publishes_tool_call_events() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    // Second call (has "tool_result" in body) → end_turn
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done after tool"));
    });
    // First call → tool_use
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let prefix = "test-toolcall";
    let session_id = "sess-toolcall-1";
    let req_id = "req-toolcall-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        None,
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "use a tool".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            let events = collect_until_done(&mut events_sub, 15).await;

            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, PromptEvent::ToolCallStarted { name, .. } if name == "unknown_tool")),
                "expected ToolCallStarted event"
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, PromptEvent::ToolCallFinished { .. })),
                "expected ToolCallFinished event"
            );
            let done = events.iter().find(|e| {
                matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn")
            });
            assert!(done.is_some(), "expected Done(end_turn) after tool call");
        })
        .await;
}
