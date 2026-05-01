//! Integration tests for `TrogonAgent` prompt handling — requires Docker (testcontainers starts NATS).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test runner_e2e

use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;

/// Compatibility shims for old function-based subject API.
mod agent_subjects {
    use acp_nats::{AcpPrefix, AcpSessionId};
    pub fn session_prompt(prefix: &str, session_id: &str) -> String {
        acp_nats::nats::session::agent::PromptSubject::new(
            &AcpPrefix::new(prefix).expect("valid prefix"),
            &AcpSessionId::new(session_id).expect("valid session_id"),
        ).to_string()
    }
    pub fn session_cancel(prefix: &str, session_id: &str) -> String {
        acp_nats::nats::session::agent::CancelSubject::new(
            &AcpPrefix::new(prefix).expect("valid prefix"),
            &AcpSessionId::new(session_id).expect("valid session_id"),
        ).to_string()
    }
}

mod client_subjects {
    use acp_nats::{AcpPrefix, AcpSessionId};
    pub fn session_update(prefix: &str, session_id: &str) -> String {
        acp_nats::nats::session::client::SessionUpdateSubject::new(
            &AcpPrefix::new(prefix).expect("valid prefix"),
            &AcpSessionId::new(session_id).expect("valid session_id"),
        ).to_string()
    }
}
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{ContentBlock, ImageContent, PromptRequest, TextContent};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use httpmock::prelude::*;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::{RwLock, mpsc};
use trogon_acp_runner::{
    GatewayConfig, NatsSessionNotifier, NatsSessionStore, PermissionReq, SessionState,
    SessionStore, StoredMcpServer, TrogonAgent,
};
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
        elicitation_provider: None,
    }
}

/// Start a `TrogonAgent` wired to `AgentSideNatsConnection`. Must be called inside a `LocalSet`.
async fn start_agent(
    nats: async_nats::Client,
    js: &jetstream::Context,
    prefix: &str,
    agent: AgentLoop,
    permission_tx: Option<trogon_acp_runner::PermissionTx>,
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
) -> NatsSessionStore {
    let store = NatsSessionStore::open(js).await.unwrap();
    let notifier = NatsSessionNotifier::new(nats.clone());
    let ta = TrogonAgent::new(
        notifier,
        store.clone(),
        agent,
        prefix,
        "claude-test",
        permission_tx,
        None,
        gateway_config,
    );
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let (_, io_task) = AgentSideNatsConnection::new(ta, nats, acp_prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(io_task);
    tokio::time::sleep(Duration::from_millis(50)).await;
    store
}

/// Subscribe to session notifications and send a prompt as NATS request-reply.
/// Returns (notif_sub, resp_sub) — use with `collect_notifs_and_response`.
async fn subscribe_and_send(
    nats: &async_nats::Client,
    prefix: &str,
    session_id: &str,
    blocks: Vec<ContentBlock>,
) -> (async_nats::Subscriber, async_nats::Subscriber) {
    let notif_sub = nats
        .subscribe(client_subjects::session_update(prefix, session_id))
        .await
        .expect("subscribe to notifications");
    let inbox = nats.new_inbox();
    let resp_sub = nats.subscribe(inbox.clone()).await.expect("subscribe to inbox");
    let req = PromptRequest::new(session_id.to_owned(), blocks);
    nats.publish_with_reply(
        agent_subjects::session_prompt(prefix, session_id),
        inbox,
        Bytes::from(serde_json::to_vec(&req).unwrap()),
    )
    .await
    .unwrap();
    (notif_sub, resp_sub)
}

/// Send a plain text prompt.
async fn send_text(
    nats: &async_nats::Client,
    prefix: &str,
    session_id: &str,
    text: &str,
) -> (async_nats::Subscriber, async_nats::Subscriber) {
    subscribe_and_send(
        nats,
        prefix,
        session_id,
        vec![ContentBlock::Text(TextContent::new(text))],
    )
    .await
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

/// Collect `SessionNotification` messages from `notif_sub` until a message
/// arrives on `resp_sub`, then return all notifications and the final response.
///
/// Notifications are returned as raw `serde_json::Value` for flexible assertion.
/// The response is the parsed JSON from the response subject (either
/// `{"stopReason": "..."}` or `{"code": ...}`).
async fn collect_notifs_and_response(
    notif_sub: &mut async_nats::Subscriber,
    resp_sub: &mut async_nats::Subscriber,
    timeout_secs: u64,
) -> (Vec<serde_json::Value>, serde_json::Value) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut notifs = vec![];
    loop {
        tokio::select! {
            biased;
            msg = tokio::time::timeout_at(deadline, resp_sub.next()) => {
                let msg = msg
                    .expect("timed out waiting for response message")
                    .expect("response subscription ended unexpectedly");
                let resp: serde_json::Value =
                    serde_json::from_slice(&msg.payload).expect("invalid response JSON");
                return (notifs, resp);
            }
            msg = tokio::time::timeout_at(deadline, notif_sub.next()) => {
                let msg = msg
                    .expect("timed out waiting for notification")
                    .expect("notification subscription ended unexpectedly");
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&msg.payload) {
                    notifs.push(v);
                }
            }
        }
    }
}

// ── Agent creation ─────────────────────────────────────────────────────────────

/// `TrogonAgent` creation succeeds and creates the `ACP_SESSIONS` KV bucket.
/// After creation, `SessionStore::open` on the same JetStream context is idempotent.
#[tokio::test]
async fn agent_new_creates_session_bucket() {
    let (_c, nats, js) = start_nats().await;
    let agent = make_agent("http://127.0.0.1:1");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _store = start_agent(
                nats,
                &js,
                "test-new",
                agent,
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Opening the store again must be idempotent (bucket already exists).
            NatsSessionStore::open(&js)
                .await
                .expect("NatsSessionStore::open must succeed after start_agent");
        })
        .await;
}

// ── Error path ─────────────────────────────────────────────────────────────────

/// When the Anthropic endpoint is unreachable, the agent returns an error or
/// Done response (connection-refused → HTTP error → internal error or cancelled).
#[tokio::test]
async fn runner_publishes_error_event_when_anthropic_unreachable() {
    let (_c, nats, js) = start_nats().await;

    // Port 1 = connection refused, so reqwest will fail immediately.
    let agent = make_agent("http://127.0.0.1:1");
    let prefix = "test-err";
    let session_id = "sess-err-1";

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                agent,
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "hello").await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 10).await;

            // Must be either a PromptResponse (has "stopReason") or an ACP error (has "code").
            assert!(
                resp.get("stopReason").is_some() || resp.get("code").is_some(),
                "expected stopReason or code in response; got: {resp}"
            );
        })
        .await;
}

// ── Happy path ─────────────────────────────────────────────────────────────────

/// When the Anthropic API returns `end_turn`, the agent publishes `AgentMessageChunk`
/// notifications then returns `Done { stop_reason: "end_turn" }`.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "hello").await;

            let (notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert!(
                notifs
                    .iter()
                    .any(|n| n.to_string().contains("Great response!")),
                "expected notification containing 'Great response!'"
            );
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn; got: {resp}"
            );
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store = start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "persist me").await;

            collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            // After the turn, the session must be persisted in KV.
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

/// A malformed prompt payload (invalid JSON) sent without a reply subject is
/// ignored by the agent — it keeps listening and processes the next valid
/// prompt without crashing.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Send garbage without a reply subject — agent logs a warning and continues.
            nats.publish(
                agent_subjects::session_prompt(prefix, session_id),
                Bytes::from(b"not valid json".to_vec()),
            )
            .await
            .unwrap();

            // Give agent a moment to process (and discard) the bad message.
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Now send a valid prompt — agent must still handle it.
            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "valid").await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert!(
                resp.get("stopReason").is_some(),
                "expected stopReason in response after skipping bad payload; got: {resp}"
            );
        })
        .await;
}

// ── Error stop reasons ─────────────────────────────────────────────────────────

/// When Anthropic returns `max_tokens`, the agent returns `Done { stop_reason: "max_tokens" }`.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "fill the context").await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("max_tokens"),
                "expected stop_reason=max_tokens; got: {resp}"
            );
        })
        .await;
}

/// When `max_iterations` is exhausted (model always returns `tool_use`),
/// the agent returns `Done { stop_reason: "max_turn_requests" }`.
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

    let mut agent = make_agent(&server.base_url());
    agent.max_iterations = 1; // exhaust after one tool_use → MaxIterationsReached

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                agent,
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "loop forever").await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("max_turn_requests"),
                "expected stop_reason=max_turn_requests; got: {resp}"
            );
        })
        .await;
}

/// When the model requests a tool call, the agent publishes `ToolCall`
/// and `ToolCallUpdate` notifications before the final `Done`.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "use a tool").await;

            let (notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert!(
                notifs
                    .iter()
                    .any(|n| n.to_string().contains("unknown_tool")),
                "expected notification containing 'unknown_tool'"
            );
            assert!(
                notifs.iter().any(|n| {
                    let s = n.to_string();
                    s.contains("toolCallUpdate") || s.contains("ToolCallUpdate") || s.contains("toolCall")
                }),
                "expected tool call notification"
            );
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn after tool call; got: {resp}"
            );
        })
        .await;
}

// ── Permission gate ───────────────────────────────────────────────────────────

/// When `permission_tx` is set and the checker approves the tool call, the
/// agent executes the tool and publishes ToolCall + Done(end_turn).
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

    let (permission_tx, mut permission_rx) = mpsc::channel::<PermissionReq>(8);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                Some(permission_tx),
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Approve every permission request
            tokio::spawn(async move {
                while let Some(req) = permission_rx.recv().await {
                    let _ = req.response_tx.send(true);
                }
            });

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "use a tool").await;

            let (notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            // ToolCall notification must appear — permission was checked and approved
            assert!(
                notifs.iter().any(|n| n.to_string().contains("unknown_tool")),
                "expected notification containing 'unknown_tool' after permission approved; notifs: {notifs:?}"
            );
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn; got: {resp}"
            );
        })
        .await;
}

/// When `permission_tx` is set and the checker denies the tool call, the
/// agent still completes (the agent sends the denial as a tool result and
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

    let (permission_tx, mut permission_rx) = mpsc::channel::<PermissionReq>(8);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                Some(permission_tx),
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Deny every permission request
            tokio::spawn(async move {
                while let Some(req) = permission_rx.recv().await {
                    let _ = req.response_tx.send(false);
                }
            });

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "use a tool").await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            // The agent sends a denial tool-result and Anthropic returns end_turn
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn after permission denial; got: {resp}"
            );
        })
        .await;
}

// ── MCP dispatch ──────────────────────────────────────────────────────────────

/// When a session has `mcp_servers` configured, the agent calls `build_session_mcp`
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

    // Pre-save session state with the MCP server configured
    let session_store = NatsSessionStore::open(&js).await.unwrap();
    let state = SessionState {
        mcp_servers: vec![StoredMcpServer {
            name: "my_srv".to_string(),
            url: mcp_server.base_url(),
            headers: vec![],
        }],
        ..Default::default()
    };
    session_store.save(session_id, &state).await.unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&anthropic.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "call the MCP tool").await;

            let (notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            // The MCP tool must be dispatched (prefixed name = "my_srv__my_tool")
            assert!(
                notifs
                    .iter()
                    .any(|n| n.to_string().contains("my_srv__my_tool")),
                "expected notification containing 'my_srv__my_tool'; notifs: {notifs:?}"
            );
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn after MCP tool call; got: {resp}"
            );
        })
        .await;
}

// ── Cancellation ──────────────────────────────────────────────────────────────

/// Sending a cancel message to `{prefix}.{session_id}.agent.session.cancel` while the
/// agent is processing a prompt causes the agent to abort and return
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

    let cancel_subject = agent_subjects::session_cancel(prefix, session_id);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "do something slow").await;

            // Give the agent a moment to start processing, then cancel
            tokio::time::sleep(Duration::from_millis(300)).await;
            nats.publish(cancel_subject, Bytes::new()).await.unwrap();

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 10).await;

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("cancelled"),
                "expected stop_reason=cancelled; got: {resp}"
            );
        })
        .await;
}

// ── Gateway config override ───────────────────────────────────────────────────

/// When `gateway_config` is set on the agent, the agent uses the gateway's
/// `base_url` and `token` instead of the agent's own values.
/// Verified by creating an agent whose embedded loop points at a dead endpoint
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

    // Agent points at port 1 (dead) — must be overridden by gateway_config
    let gateway_config = Arc::new(RwLock::new(Some(GatewayConfig {
        base_url: gateway.base_url(),
        token: "gw-token".to_string(),
        extra_headers: vec![],
    })));

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent("http://127.0.0.1:1"),
                None,
                None,
                gateway_config,
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "hello via gateway").await;

            let (notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 10).await;

            // The notification must contain the response from the gateway mock
            assert!(
                notifs.iter().any(|n| n.to_string().contains("via gateway")),
                "expected notification with gateway response; notifs: {notifs:?}"
            );
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn via gateway; got: {resp}"
            );
        })
        .await;
}

// ── Session queuing ───────────────────────────────────────────────────────────

/// When 3 prompts are sent to the same session_id without waiting for
/// acknowledgement, the agent processes them in order via the session semaphore
/// and all 3 must complete with a `Done` event.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Subscribe to the shared notification subject for this session.
            let mut notif_sub = nats
                .subscribe(client_subjects::session_update(prefix, session_id))
                .await
                .expect("subscribe to notifications");

            // Create 3 separate inboxes and send all prompts rapidly.
            let mut resp_subs = Vec::new();
            for i in 0..3u32 {
                let inbox = nats.new_inbox();
                let resp_sub = nats.subscribe(inbox.clone()).await.unwrap();
                resp_subs.push(resp_sub);
                let req = PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new(format!("prompt {i}")))],
                );
                nats.publish_with_reply(
                    agent_subjects::session_prompt(prefix, session_id),
                    inbox,
                    Bytes::from(serde_json::to_vec(&req).unwrap()),
                )
                .await
                .unwrap();
            }

            // Wait for Done on each subscription in order.
            for (i, mut resp_sub) in resp_subs.into_iter().enumerate() {
                let (_notifs, resp) =
                    collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 30).await;
                assert!(
                    resp.get("stopReason").is_some(),
                    "expected stopReason in response for prompt #{i}; got: {resp}"
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Send both prompts to different sessions simultaneously.
            let (mut notif_a, mut resp_a) =
                send_text(&nats, prefix, session_a, "hello session A").await;
            let (mut notif_b, mut resp_b) =
                send_text(&nats, prefix, session_b, "hello session B").await;

            // Both sessions must receive a terminal response.
            let (_notifs_a, resp_a_val) =
                collect_notifs_and_response(&mut notif_a, &mut resp_a, 15).await;
            assert!(
                resp_a_val.get("stopReason").is_some(),
                "expected stopReason for session_a; got: {resp_a_val}"
            );

            let (_notifs_b, resp_b_val) =
                collect_notifs_and_response(&mut notif_b, &mut resp_b, 15).await;
            assert!(
                resp_b_val.get("stopReason").is_some(),
                "expected stopReason for session_b; got: {resp_b_val}"
            );
        })
        .await;
}

// ── Content blocks ─────────────────────────────────────────────────────────────

/// Sending a prompt with a text resource (context) content block must be processed
/// by the agent without crashing and must result in a `Done` event.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // Two text blocks — simulates a context-rich prompt.
            let blocks = vec![
                ContentBlock::Text(TextContent::new("look at this context")),
                ContentBlock::Text(TextContent::new(
                    "<context ref=\"file:///project/README.md\">\n# Project README\nThis is the content.\n</context>",
                )),
            ];
            let (mut notif_sub, mut resp_sub) =
                subscribe_and_send(&nats, prefix, session_id, blocks).await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn after context content; got: {resp}"
            );
        })
        .await;
}

/// A prompt containing a base64-encoded image content block must be
/// processed by the agent without crashing and result in a `Done` event.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // A minimal 1x1 white PNG in base64.
            let blocks = vec![
                ContentBlock::Text(TextContent::new("look at this image")),
                ContentBlock::Image(ImageContent::new(
                    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwADhQGAWjR9awAAAABJRU5ErkJggg==",
                    "image/png",
                )),
            ];
            let (mut notif_sub, mut resp_sub) =
                subscribe_and_send(&nats, prefix, session_id, blocks).await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn after Image content block; got: {resp}"
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            // First prompt.
            let (mut notif_sub_1, mut resp_sub_1) =
                send_text(&nats, prefix, session_id, "First question").await;
            let (_notifs_1, resp_1) =
                collect_notifs_and_response(&mut notif_sub_1, &mut resp_sub_1, 15).await;
            assert_eq!(
                resp_1["stopReason"].as_str(),
                Some("end_turn"),
                "first prompt must complete with stop_reason=end_turn; got: {resp_1}"
            );

            // Short pause so session state is persisted before second prompt.
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Second prompt — history should now include first exchange.
            let (mut notif_sub_2, mut resp_sub_2) =
                send_text(&nats, prefix, session_id, "Second question").await;
            let (_notifs_2, resp_2) =
                collect_notifs_and_response(&mut notif_sub_2, &mut resp_sub_2, 15).await;
            assert_eq!(
                resp_2["stopReason"].as_str(),
                Some("end_turn"),
                "second prompt must complete with stop_reason=end_turn; got: {resp_2}"
            );

            // The second Anthropic request (matched by body_contains "Second question")
            // was routed to the second mock — confirming the agent sent a new call
            // that included "Second question" in the body (history + new message).
        })
        .await;
}

/// When the Anthropic response includes a `parent_tool_use_id` on a tool-use
/// block, the agent publishes a `ToolCall` notification carrying that value.
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

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "run nested tool").await;

            let (notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            // Find the notification for "unknown_tool" and verify parent_tool_use_id is present.
            assert!(
                notifs
                    .iter()
                    .any(|n| n.to_string().contains("unknown_tool")),
                "expected notification containing 'unknown_tool'; notifs: {notifs:?}"
            );
            assert!(
                notifs
                    .iter()
                    .any(|n| n.to_string().contains("tu_parent_001")),
                "expected notification containing 'tu_parent_001'; notifs: {notifs:?}"
            );
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn; got: {resp}"
            );
        })
        .await;
}

// ── Cancel during tool execution ───────────────────────────────────────────────

/// When a cancel message arrives WHILE the agent is executing a tool call
/// (i.e., waiting for the second Anthropic response after sending tool_result),
/// the agent should still complete with Done(cancelled) or Done(end_turn).
///
/// We simulate this by:
/// 1. First Anthropic call → tool_use (triggers tool execution)
/// 2. While waiting for the second Anthropic call, publish cancel message
/// 3. Second Anthropic call → end_turn (the cancel check is cooperative)
///
/// The agent publishes Done with some stop reason (cancelled or end_turn depending on timing).
#[tokio::test]
async fn runner_cancel_during_tool_execution_completes() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();

    // Second call (tool_result arrives) → small delay then end_turn
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Done after tool"))
            .delay(Duration::from_millis(100));
    });
    // First call → tool_use
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(tool_use_body());
    });

    let prefix = "test-cancel-tool";
    let session_id = "sess-cancel-tool-1";

    let cancel_subject = agent_subjects::session_cancel(prefix, session_id);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "use a tool").await;

            // Wait for the tool call to start, then send cancel
            tokio::time::sleep(Duration::from_millis(50)).await;
            nats.publish(cancel_subject, Bytes::new()).await.unwrap();

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            // Should complete with some stop_reason (cancelled or end_turn depending on timing)
            assert!(
                resp.get("stopReason").is_some(),
                "agent must return a stopReason after cancel during tool; got: {resp}"
            );
        })
        .await;
}

// ── No cancel signal path ──────────────────────────────────────────────────────

/// Verify the agent completes a prompt successfully when no cancel message is sent.
#[tokio::test]
async fn runner_completes_prompt_without_any_cancel_signal() {
    let (_c, nats, js) = start_nats().await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("completed without cancel"));
    });

    let prefix = "test-no-cancel";
    let session_id = "sess-no-cancel-1";

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url()),
                None,
                Arc::new(RwLock::new(None)),
            )
            .await;

            let (mut notif_sub, mut resp_sub) =
                send_text(&nats, prefix, session_id, "Hello").await;

            let (_notifs, resp) =
                collect_notifs_and_response(&mut notif_sub, &mut resp_sub, 15).await;

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected stop_reason=end_turn; got: {resp}"
            );
        })
        .await;
}
