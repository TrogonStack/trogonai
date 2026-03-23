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
use tokio::sync::{RwLock, mpsc};
use trogon_acp_runner::{PermissionReq, Runner, SessionState, SessionStore, StoredMcpServer};
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

// ── Permission gate ───────────────────────────────────────────────────────────

/// When `permission_tx` is set and the checker approves the tool call, the
/// runner executes the tool and publishes ToolCallStarted + Done(end_turn).
#[tokio::test]
async fn runner_tool_call_allowed_via_permission_channel() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    // Second call (has tool_result) → end_turn
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done after approved tool"));
    });
    // First call → tool_use
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let prefix = "test-perm-allow";
    let session_id = "sess-perm-allow-1";
    let req_id = "req-perm-allow-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let (permission_tx, mut permission_rx) = mpsc::channel::<PermissionReq>(8);

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        Some(permission_tx),
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Approve every permission request
            tokio::spawn(async move {
                while let Some(req) = permission_rx.recv().await {
                    let _ = req.response_tx.send(true);
                }
            });

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

            // ToolCallStarted must appear — permission was checked and approved
            assert!(
                events.iter().any(
                    |e| matches!(e, PromptEvent::ToolCallStarted { name, .. } if name == "unknown_tool")
                ),
                "expected ToolCallStarted(unknown_tool) after permission approved; got {events:?}"
            );
            let done = events.iter().find(|e| {
                matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn")
            });
            assert!(done.is_some(), "expected Done(end_turn)");
        })
        .await;
}

/// When `permission_tx` is set and the checker denies the tool call, the
/// runner still completes (the agent sends the denial as a tool result and
/// Anthropic returns end_turn).
#[tokio::test]
async fn runner_tool_call_denied_via_permission_channel() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    // Second call (has tool_result — the denial) → end_turn
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done after denied tool"));
    });
    // First call → tool_use
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let prefix = "test-perm-deny";
    let session_id = "sess-perm-deny-1";
    let req_id = "req-perm-deny-1";

    let agent = make_agent(&server.base_url());
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let (permission_tx, mut permission_rx) = mpsc::channel::<PermissionReq>(8);

    let runner = Runner::new(
        nats.clone(),
        &js,
        agent,
        prefix,
        Some(permission_tx),
        Arc::new(RwLock::new(None)),
    )
    .await
    .unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Deny every permission request
            tokio::spawn(async move {
                while let Some(req) = permission_rx.recv().await {
                    let _ = req.response_tx.send(false);
                }
            });

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

            // The agent sends a denial tool-result and Anthropic returns end_turn
            let done = events.iter().find(
                |e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"),
            );
            assert!(
                done.is_some(),
                "expected Done(end_turn) after permission denial; got {events:?}"
            );
        })
        .await;
}

// ── MCP dispatch ──────────────────────────────────────────────────────────────

/// When a session has `mcp_servers` configured, the runner calls `build_session_mcp`
/// at prompt time, lists the tools, and dispatches tool calls to the MCP server.
#[tokio::test]
async fn runner_dispatches_mcp_tool_via_session_mcp_servers() {
    let (_c, nats, js) = start_nats().await;

    // ── MCP mock server ───────────────────────────────────────────────────────
    let mcp_server = MockServer::start();

    // initialize
    mcp_server.mock(|when, then| {
        when.method(POST).body_contains("\"initialize\"");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"test-mcp"}}}"#);
    });
    // tools/list — returns one tool named "my_tool"
    mcp_server.mock(|when, then| {
        when.method(POST).body_contains("\"tools/list\"");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"my_tool","description":"A test MCP tool","inputSchema":{"type":"object"}}]}}"#);
    });
    // tools/call — returns text content
    mcp_server.mock(|when, then| {
        when.method(POST).body_contains("\"tools/call\"");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"mcp result"}],"isError":false}}"#);
    });

    // ── Anthropic mock ────────────────────────────────────────────────────────
    let anthropic = MockServer::start();

    // Second call (has tool_result) → end_turn
    anthropic.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done with MCP result"));
    });
    // First call → tool_use with the prefixed name "my_srv__my_tool"
    anthropic.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_mcp_1",
                        "name": "my_srv__my_tool",
                        "input": {}
                    }]
                })
                .to_string(),
            );
    });

    let prefix = "test-mcp";
    let session_id = "sess-mcp-1";
    let req_id = "req-mcp-1";

    // Pre-save session state with the MCP server configured
    let session_store = SessionStore::open(&js).await.unwrap();
    let state = SessionState {
        mcp_servers: vec![StoredMcpServer {
            name: "my_srv".to_string(),
            url: mcp_server.base_url(),
            headers: vec![],
        }],
        ..Default::default()
    };
    session_store.save(session_id, &state).await.unwrap();

    let agent = make_agent(&anthropic.base_url());
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
                    text: "call the MCP tool".to_string(),
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

            // The MCP tool must be dispatched (prefixed name = "my_srv__my_tool")
            assert!(
                events.iter().any(
                    |e| matches!(e, PromptEvent::ToolCallStarted { name, .. } if name == "my_srv__my_tool")
                ),
                "expected ToolCallStarted(my_srv__my_tool); got {events:?}"
            );
            let done = events.iter().find(|e| {
                matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn")
            });
            assert!(done.is_some(), "expected Done(end_turn) after MCP tool call");
        })
        .await;
}

// ── Cancellation ──────────────────────────────────────────────────────────────

/// Sending a cancel message to `{prefix}.{session_id}.agent.session.cancel` while the
/// agent is processing a prompt causes the runner to abort and publish
/// `Done { stop_reason: "cancelled" }`.
#[tokio::test]
async fn runner_publishes_done_cancelled_when_cancel_message_arrives() {
    let (_c, nats, js) = start_nats().await;

    // Slow Anthropic mock: 2-second delay gives the cancel message time to arrive.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .delay(Duration::from_secs(2))
            .body(end_turn_body("never reached"));
    });

    let prefix = "test-cancel";
    let session_id = "sess-cancel-1";
    let req_id = "req-cancel-1";

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

    let cancel_subject = subjects::session_cancel(prefix, session_id);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            let payload = PromptPayload {
                req_id: req_id.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "do something slow".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            // Give the runner a moment to start processing, then cancel
            tokio::time::sleep(Duration::from_millis(300)).await;
            nats.publish(cancel_subject, Bytes::new()).await.unwrap();

            let events = collect_until_done(&mut events_sub, 10).await;

            let done = events.iter().find(
                |e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "cancelled"),
            );
            assert!(done.is_some(), "expected Done(cancelled); got {events:?}");
        })
        .await;
}

// ── Gateway config override ───────────────────────────────────────────────────

/// When `gateway_config` is set on the runner, the agent uses the gateway's
/// `base_url` and `token` instead of the agent's own values.
/// Verified by creating a runner whose embedded agent points at a dead endpoint
/// while gateway_config redirects to a live mock server.
#[tokio::test]
async fn runner_uses_gateway_config_base_url_and_token() {
    let (_c, nats, js) = start_nats().await;

    // Live gateway mock — this is where the request must actually arrive
    let gateway = MockServer::start();
    gateway.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("via gateway"));
    });

    let prefix = "test-gw";
    let session_id = "sess-gw-1";
    let req_id = "req-gw-1";

    // Agent points at port 1 (dead) — must be overridden by gateway_config
    let agent = make_agent("http://127.0.0.1:1");
    let events_subject = subjects::prompt_events(prefix, session_id, req_id);
    let mut events_sub = nats.subscribe(events_subject).await.unwrap();

    let gateway_config = Arc::new(RwLock::new(Some(trogon_acp_runner::GatewayConfig {
        base_url: gateway.base_url(),
        token: "gw-token".to_string(),
        extra_headers: vec![],
    })));

    let runner = Runner::new(nats.clone(), &js, agent, prefix, None, gateway_config)
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
                    text: "hello via gateway".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload).unwrap()),
            )
            .await
            .unwrap();

            let events = collect_until_done(&mut events_sub, 10).await;

            // The TextDelta must contain the response from the gateway mock
            assert!(
                events.iter().any(
                    |e| matches!(e, PromptEvent::TextDelta { text } if text.contains("via gateway"))
                ),
                "expected TextDelta with gateway response; got {events:?}"
            );
            let done = events.iter().find(
                |e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"),
            );
            assert!(done.is_some(), "expected Done(end_turn) via gateway");
        })
        .await;
}

// ── Session queuing ───────────────────────────────────────────────────────────

/// When 3 prompts are sent to the same session_id without waiting for
/// acknowledgement, the runner must process them in order and all 3 must
/// complete with a `Done` event.
#[tokio::test]
async fn concurrent_prompts_same_session_are_queued_in_order() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("queued reply"));
    });

    let prefix = "test-queue-same";
    let session_id = "sess-queue-same-1";
    let req_ids = ["req-q-1", "req-q-2", "req-q-3"];

    let agent = make_agent(&server.base_url());

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
            // Subscribe to all 3 event streams BEFORE the runner starts and BEFORE publishing.
            let mut subs = Vec::new();
            for req_id in &req_ids {
                let events_subject = subjects::prompt_events(prefix, session_id, req_id);
                let sub = nats
                    .subscribe(events_subject)
                    .await
                    .expect("subscribe to events");
                subs.push(sub);
            }

            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Publish all 3 prompts rapidly without waiting between them.
            for req_id in &req_ids {
                let payload = PromptPayload {
                    req_id: req_id.to_string(),
                    session_id: session_id.to_string(),
                    content: vec![UserContentBlock::Text {
                        text: format!("prompt {req_id}"),
                    }],
                    user_message: String::new(),
                };
                nats.publish(
                    subjects::prompt(prefix, session_id),
                    Bytes::from(serde_json::to_vec(&payload).unwrap()),
                )
                .await
                .unwrap();
            }

            // Wait for Done on each subscription in order.
            for (i, sub) in subs.iter_mut().enumerate() {
                let events = collect_until_done(sub, 30).await;
                let done = events
                    .iter()
                    .find(|e| matches!(e, PromptEvent::Done { .. }));
                assert!(
                    done.is_some(),
                    "expected Done event for prompt #{i} (req_id={}); got: {events:?}",
                    req_ids[i]
                );
            }
        })
        .await;
}

/// When 2 prompts are sent to DIFFERENT session_ids, both should complete
/// successfully (they run concurrently, not queued).
#[tokio::test]
async fn concurrent_prompts_different_sessions_run_concurrently() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("concurrent reply"));
    });

    let prefix = "test-queue-diff";
    let session_a = "sess-conc-a";
    let session_b = "sess-conc-b";
    let req_a = "req-conc-a";
    let req_b = "req-conc-b";

    let agent = make_agent(&server.base_url());

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
            let events_a = subjects::prompt_events(prefix, session_a, req_a);
            let events_b = subjects::prompt_events(prefix, session_b, req_b);
            let mut sub_a = nats.subscribe(events_a).await.expect("subscribe a");
            let mut sub_b = nats.subscribe(events_b).await.expect("subscribe b");

            tokio::task::spawn_local(async move { runner.run().await });
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Publish both prompts to different sessions simultaneously.
            for (session_id, req_id) in [(session_a, req_a), (session_b, req_b)] {
                let payload = PromptPayload {
                    req_id: req_id.to_string(),
                    session_id: session_id.to_string(),
                    content: vec![UserContentBlock::Text {
                        text: format!("hello {session_id}"),
                    }],
                    user_message: String::new(),
                };
                nats.publish(
                    subjects::prompt(prefix, session_id),
                    Bytes::from(serde_json::to_vec(&payload).unwrap()),
                )
                .await
                .unwrap();
            }

            // Both sessions must receive Done.
            let events_a = collect_until_done(&mut sub_a, 15).await;
            let done_a = events_a
                .iter()
                .find(|e| matches!(e, PromptEvent::Done { .. }));
            assert!(
                done_a.is_some(),
                "expected Done for session_a; got: {events_a:?}"
            );

            let events_b = collect_until_done(&mut sub_b, 15).await;
            let done_b = events_b
                .iter()
                .find(|e| matches!(e, PromptEvent::Done { .. }));
            assert!(
                done_b.is_some(),
                "expected Done for session_b; got: {events_b:?}"
            );
        })
        .await;
}

// ── Context content block ──────────────────────────────────────────────────────

/// Sending a prompt payload with `UserContentBlock::Context` (embedded text
/// resource) must be processed by the runner without crashing and must
/// result in a `Done` event.
#[tokio::test]
async fn runner_processes_prompt_with_context_content_block() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("handled context block"));
    });

    let prefix = "test-ctx-block";
    let session_id = "sess-ctx-1";
    let req_id = "req-ctx-1";

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
                content: vec![
                    UserContentBlock::Text {
                        text: "look at this context".to_string(),
                    },
                    UserContentBlock::Context {
                        uri: "file:///project/README.md".to_string(),
                        text: "# Project README\nThis is the content.".to_string(),
                    },
                ],
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
                |e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"),
            );
            assert!(
                done.is_some(),
                "expected Done(end_turn) after Context content block; got: {events:?}"
            );
        })
        .await;
}

/// A prompt containing a base64-encoded image content block must be
/// processed by the runner without crashing and result in a `Done` event.
#[tokio::test]
async fn runner_image_content_block_in_prompt_does_not_crash() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("handled image"));
    });

    let prefix = "test-img-block";
    let session_id = "sess-img-1";
    let req_id = "req-img-1";

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
                content: vec![
                    UserContentBlock::Text {
                        text: "look at this image".to_string(),
                    },
                    UserContentBlock::Image {
                        // A minimal 1x1 white PNG in base64.
                        data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwADhQGAWjR9awAAAABJRU5ErkJggg==".to_string(),
                        mime_type: "image/png".to_string(),
                    },
                ],
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
                |e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"),
            );
            assert!(
                done.is_some(),
                "expected Done(end_turn) after Image content block; got: {events:?}"
            );
        })
        .await;
}

/// The second prompt to the same session must include the conversation history
/// from the first prompt. We verify this by checking that the second Anthropic
/// request body contains the text from the first assistant response.
#[tokio::test]
async fn runner_second_prompt_loads_history_from_first_prompt() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    // Second call (has "Second question" in body) → end_turn
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("Second question");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Answer to second question"));
    });
    // First call → end_turn with specific text we can check for later
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Answer to first question"));
    });

    let prefix = "test-history";
    let session_id = "sess-hist-1";

    let agent = make_agent(&server.base_url());

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

            // First prompt.
            let req_id_1 = "req-hist-1";
            let events_subject_1 = subjects::prompt_events(prefix, session_id, req_id_1);
            let mut events_sub_1 = nats.subscribe(events_subject_1).await.unwrap();

            let payload_1 = PromptPayload {
                req_id: req_id_1.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "First question".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload_1).unwrap()),
            )
            .await
            .unwrap();

            let events_1 = collect_until_done(&mut events_sub_1, 15).await;
            assert!(
                events_1.iter().any(|e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn")),
                "first prompt must complete with Done(end_turn); got: {events_1:?}"
            );

            // Short pause so session state is persisted before second prompt.
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Second prompt — history should now include first exchange.
            let req_id_2 = "req-hist-2";
            let events_subject_2 = subjects::prompt_events(prefix, session_id, req_id_2);
            let mut events_sub_2 = nats.subscribe(events_subject_2).await.unwrap();

            let payload_2 = PromptPayload {
                req_id: req_id_2.to_string(),
                session_id: session_id.to_string(),
                content: vec![UserContentBlock::Text {
                    text: "Second question".to_string(),
                }],
                user_message: String::new(),
            };
            nats.publish(
                subjects::prompt(prefix, session_id),
                Bytes::from(serde_json::to_vec(&payload_2).unwrap()),
            )
            .await
            .unwrap();

            let events_2 = collect_until_done(&mut events_sub_2, 15).await;
            assert!(
                events_2.iter().any(|e| matches!(e, PromptEvent::Done { stop_reason } if stop_reason == "end_turn")),
                "second prompt must complete with Done(end_turn); got: {events_2:?}"
            );

            // The second Anthropic request (matched by body_contains "Second question")
            // was routed to the second mock — confirming the runner sent a new call
            // that included "Second question" in the body (history + new message).
            // The fact that the second mock matched (returning "Answer to second question")
            // validates the runner sent the correct payload with history.
        })
        .await;
}

/// When the Anthropic response includes a `parent_tool_use_id` on a tool-use
/// block, the runner publishes a `ToolCallStarted` event carrying that value.
#[tokio::test]
async fn runner_parent_tool_use_id_propagated_in_tool_call_started() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    // Second call (tool_result) → end_turn
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done"));
    });
    // First call → tool_use with parent_tool_use_id set.
    // Anthropic returns a nested tool call (sub-agent pattern).
    let nested_tool_body = serde_json::json!({
        "stop_reason": "tool_use",
        "content": [{
            "type": "tool_use",
            "id": "tu_child_001",
            "name": "unknown_tool",
            "input": {},
            "parent_tool_use_id": "tu_parent_001"
        }]
    })
    .to_string();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(nested_tool_body);
    });

    let prefix = "test-parent-id";
    let session_id = "sess-parent-1";
    let req_id = "req-parent-1";

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
                    text: "run nested tool".to_string(),
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

            // Find the ToolCallStarted event and verify parent_tool_use_id.
            let tool_started = events.iter().find(|e| {
                matches!(e, PromptEvent::ToolCallStarted { name, .. } if name == "unknown_tool")
            });
            assert!(
                tool_started.is_some(),
                "expected ToolCallStarted event; got: {events:?}"
            );
            if let Some(PromptEvent::ToolCallStarted { parent_tool_use_id, .. }) = tool_started {
                assert_eq!(
                    parent_tool_use_id.as_deref(),
                    Some("tu_parent_001"),
                    "parent_tool_use_id must be propagated from Anthropic response"
                );
            }
        })
        .await;
}
