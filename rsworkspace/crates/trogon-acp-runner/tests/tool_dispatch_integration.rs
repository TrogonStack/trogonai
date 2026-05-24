//! Integration tests verifying that the ACP runner correctly dispatches real
//! tool calls (list_dir, git_status, git_diff, git_log, search_files) via its
//! wire format (Anthropic SSE tool_use → dispatch_tool → tool_result in the
//! second Anthropic API call).
//!
//! Requires Docker (testcontainers NATS JetStream + httpmock Anthropic API).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test tool_dispatch_integration

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{ContentBlock, PromptRequest, TextContent};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use httpmock::prelude::*;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionNotifier, NatsSessionStore, TrogonAgent,
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

fn make_agent(base_url: &str, cwd: &str) -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        anthropic_base_url: Some(base_url.to_string()),
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: cwd.to_string(),
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

async fn start_agent(
    nats: async_nats::Client,
    js: &jetstream::Context,
    prefix: &str,
    agent: AgentLoop,
) {
    let store = NatsSessionStore::open(js).await.unwrap();
    let notifier = NatsSessionNotifier::new(nats.clone());
    let ta = TrogonAgent::new(
        notifier,
        store,
        agent,
        prefix,
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let (_, io_task) = AgentSideNatsConnection::new(ta, nats, acp_prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(io_task);
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Send a prompt via NATS and collect the PromptResponse.
async fn prompt_and_wait(
    nats: &async_nats::Client,
    prefix: &str,
    session_id: &str,
    text: &str,
    timeout_secs: u64,
) -> serde_json::Value {
    let inbox = nats.new_inbox();
    let mut resp_sub = nats.subscribe(inbox.clone()).await.unwrap();

    let req = PromptRequest::new(
        session_id.to_owned(),
        vec![ContentBlock::Text(TextContent::new(text))],
    );
    nats.publish_with_reply(
        format!("{prefix}.session.{session_id}.agent.prompt"),
        inbox,
        Bytes::from(serde_json::to_vec(&req).unwrap()),
    )
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    tokio::time::timeout_at(deadline, resp_sub.next())
        .await
        .expect("timed out waiting for prompt response")
        .map(|msg| serde_json::from_slice::<serde_json::Value>(&msg.payload).unwrap())
        .expect("response subscription closed")
}

fn sse_event(event_type: &str, data: serde_json::Value) -> String {
    format!("event: {event_type}\ndata: {}\n\n", serde_json::to_string(&data).unwrap())
}

fn sse_tool_use_stream(tool_id: &str, tool_name: &str, input: serde_json::Value) -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": tool_id, "name": tool_name}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": serde_json::to_string(&input).unwrap()
            }
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn sse_end_turn_stream(text: &str) -> String {
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
            "usage": {"output_tokens": 8}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

fn init_git_repo(dir: &std::path::Path) {
    Command::new("git").args(["init"]).current_dir(dir).output().unwrap();
    Command::new("git").args(["config", "user.email", "t@t.com"]).current_dir(dir).output().unwrap();
    Command::new("git").args(["config", "user.name", "T"]).current_dir(dir).output().unwrap();
    std::fs::write(dir.join("init.txt"), "init").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(dir).output().unwrap();
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `list_dir` tool dispatched via ACP wire format — second API call body contains
/// "tool_result", and the prompt completes with end_turn.
///
/// Verification: the mock for the second call requires `body_contains("alpha.txt")`.
/// If list_dir doesn't return the directory entries, that mock won't match and the
/// test will time out / fail.
#[tokio::test]
async fn acp_list_dir_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "a").unwrap();
    std::fs::write(dir.path().join("beta.txt"), "b").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-listdir";
    let session_id = "sess-acp-listdir-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();

    // Second call: must include tool_result with directory entries
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("alpha.txt");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("listed directory"));
    });
    // First call: returns list_dir tool_use
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_listdir_001",
                "list_dir",
                serde_json::json!({"path": "."}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;

            let resp = prompt_and_wait(&nats, prefix, session_id, "list dir", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after list_dir dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "second mock (tool_result with alpha.txt) must be hit once");
}

/// `git_status` dispatched via ACP wire format — second API call body contains
/// tool_result without "Unknown tool".
#[tokio::test]
async fn acp_git_status_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    init_git_repo(dir.path());

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-gitstatus";
    let session_id = "sess-acp-gitstatus-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    // Second call: must include tool_result (git_status never returns "Unknown tool")
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("got status"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_gitstatus_001",
                "git_status",
                serde_json::json!({}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "git status", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after git_status dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "second mock (tool_result) must be hit once for git_status");
}

/// `git_diff` dispatched via ACP wire format — diff output appears in tool_result.
#[tokio::test]
async fn acp_git_diff_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    init_git_repo(dir.path());
    std::fs::write(dir.path().join("init.txt"), "changed content").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-gitdiff";
    let session_id = "sess-acp-gitdiff-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    // Second call: must include tool_result (git_diff output includes "init.txt")
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("init.txt");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("got diff"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_gitdiff_001",
                "git_diff",
                serde_json::json!({}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "git diff", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after git_diff dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "second mock (tool_result with init.txt) must be hit once for git_diff");
}

/// `git_log` dispatched via ACP wire format — commit history appears in tool_result.
#[tokio::test]
async fn acp_git_log_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    init_git_repo(dir.path());

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-gitlog";
    let session_id = "sess-acp-gitlog-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    // Second call: must include tool_result with "initial" commit message
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("initial");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("got log"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_gitlog_001",
                "git_log",
                serde_json::json!({}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "git log", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after git_log dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "second mock (tool_result with 'initial') must be hit once for git_log");
}

/// `search_files` dispatched via ACP wire format — matched line appears in tool_result.
#[tokio::test]
async fn acp_search_files_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("source.txt"), "marker_string_xyz\nother line\n").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-search";
    let session_id = "sess-acp-search-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    // Second call: must include tool_result with the matched string
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("marker_string_xyz");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("found match"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_search_001",
                "search_files",
                serde_json::json!({"pattern": "marker_string_xyz", "path": "."}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "search", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after search_files dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "second mock (tool_result with marker_string_xyz) must be hit once");
}
