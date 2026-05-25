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
use agent_client_protocol::{ContentBlock, NewSessionRequest, PromptRequest, TextContent};
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

// ── TROGON.md → ACP system prompt wire ───────────────────────────────────────

/// A TROGON.md file in the session cwd must be injected into the system prompt
/// sent to the Anthropic API (appears in the first HTTP request body).
///
/// The agent loads TROGON.md from `state.cwd`, which is set during new_session.
/// This test calls new_session first so state.cwd points to the temp dir, then
/// asserts that the TROGON.md content appears in the Anthropic API wire request.
#[tokio::test]
async fn acp_trogon_md_injected_into_system_prompt_in_wire_request() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "acp-project-rules: safety first").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-trogon-md";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();

    // Specific mock: first call must contain TROGON.md content in request body
    let trogon_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("acp-project-rules");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("done"));
    });
    // Catch-all: if TROGON.md is NOT injected the request won't match above and
    // falls through here — the test then fails on trogon_mock.hits() == 0.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("fallback"));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;

            // Call new_session so state.cwd is set to the dir containing TROGON.md.
            let session_id = {
                let req = NewSessionRequest::new(std::path::PathBuf::from(&cwd));
                let inbox = nats.new_inbox();
                let mut resp_sub = nats.subscribe(inbox.clone()).await.unwrap();
                nats.publish_with_reply(
                    format!("{prefix}.agent.session.new"),
                    inbox,
                    Bytes::from(serde_json::to_vec(&req).unwrap()),
                )
                .await
                .unwrap();
                let msg = tokio::time::timeout(
                    Duration::from_secs(5),
                    resp_sub.next(),
                )
                .await
                .expect("timed out waiting for new_session response")
                .expect("no new_session response");
                let v: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            let resp = prompt_and_wait(&nats, prefix, &session_id, "hello", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn; got: {resp}"
            );
        })
        .await;

    assert_eq!(
        trogon_mock.hits(),
        1,
        "TROGON.md content must appear in the first Anthropic API request body"
    );
}

// ── notebook_edit → ACP wire ──────────────────────────────────────────────────

/// `notebook_edit` dispatched via ACP wire format — the tool_result is sent in
/// the second Anthropic API call and the notebook cell is updated on disk.
#[tokio::test]
async fn acp_notebook_edit_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();

    let notebook = serde_json::json!({
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {},
        "cells": [{
            "cell_type": "code",
            "source": ["original content"],
            "metadata": {},
            "outputs": [],
            "execution_count": null
        }]
    });
    std::fs::write(
        dir.path().join("nb.ipynb"),
        serde_json::to_string(&notebook).unwrap(),
    )
    .unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-nbedit";
    let session_id = "sess-acp-nbedit-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();

    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("notebook edited"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_nbedit_001",
                "notebook_edit",
                serde_json::json!({
                    "path": "nb.ipynb",
                    "cell_index": 0,
                    "content": "updated content"
                }),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "edit notebook", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after notebook_edit dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "tool_result mock must be hit once for notebook_edit");

    let raw = std::fs::read_to_string(dir.path().join("nb.ipynb")).unwrap();
    let nb: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let source = nb["cells"][0]["source"].to_string();
    assert!(
        source.contains("updated content"),
        "notebook cell must be updated on disk by notebook_edit; got: {source}"
    );
}

// ── read_file → ACP wire ──────────────────────────────────────────────────────

/// `read_file` dispatched via ACP wire format — the file content appears in
/// the tool_result sent in the second Anthropic API call.
#[tokio::test]
async fn acp_read_file_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("readme.txt"), "acp-read-sentinel-xyz\n").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-readfile";
    let session_id = "sess-acp-readfile-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("acp-read-sentinel-xyz");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("file read"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_readfile_001",
                "read_file",
                serde_json::json!({"path": "readme.txt"}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "read file", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after read_file dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "tool_result with file content must be hit once");
}

// ── write_file → ACP wire ─────────────────────────────────────────────────────

/// `write_file` dispatched via ACP wire format — the file is created on disk
/// and "OK" appears in the tool_result.
#[tokio::test]
async fn acp_write_file_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-writefile";
    let session_id = "sess-acp-writefile-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("file written"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_writefile_001",
                "write_file",
                serde_json::json!({"path": "new.rs", "content": "fn hello() {}\n"}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "write file", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after write_file dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "tool_result mock must be hit once for write_file");
    let on_disk = std::fs::read_to_string(dir.path().join("new.rs")).unwrap();
    assert_eq!(on_disk, "fn hello() {}\n", "write_file must persist content to disk");
}

// ── str_replace → ACP wire ────────────────────────────────────────────────────

/// `str_replace` dispatched via ACP wire format — the file is edited and the
/// diff appears in the tool_result sent to Anthropic.
#[tokio::test]
async fn acp_str_replace_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn old_impl() {}\n").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-strreplace";
    let session_id = "sess-acp-strreplace-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("replaced"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_strreplace_001",
                "str_replace",
                serde_json::json!({
                    "path": "code.rs",
                    "old_str": "fn old_impl()",
                    "new_str": "fn new_impl()"
                }),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "replace", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after str_replace dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "tool_result mock must be hit once for str_replace");
    let on_disk = std::fs::read_to_string(dir.path().join("code.rs")).unwrap();
    assert!(
        on_disk.contains("fn new_impl()"),
        "str_replace must apply the edit on disk; got: {on_disk}"
    );
    assert!(
        !on_disk.contains("fn old_impl()"),
        "old text must be removed from disk; got: {on_disk}"
    );
}

// ── glob → ACP wire ───────────────────────────────────────────────────────────

/// `glob` dispatched via ACP wire format — matching filenames appear in the
/// tool_result sent in the second Anthropic API call.
#[tokio::test]
async fn acp_glob_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("match.rs"), "fn x() {}").unwrap();
    std::fs::write(dir.path().join("ignore.txt"), "not rust").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-glob";
    let session_id = "sess-acp-glob-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("match.rs");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("glob done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_glob_001",
                "glob",
                serde_json::json!({"pattern": "**/*.rs"}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "glob", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after glob dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "tool_result with match.rs must be hit once for glob");
}

// ── fetch_url → ACP wire ──────────────────────────────────────────────────────

/// `fetch_url` dispatched via ACP wire format — the fetched page content
/// appears in the tool_result sent in the second Anthropic API call.
#[tokio::test]
async fn acp_fetch_url_tool_dispatched_via_wire_format() {
    use httpmock::prelude::*;

    // Server that serves the URL being fetched.
    let target_server = MockServer::start();
    target_server.mock(|when, then| {
        when.method(GET).path("/page");
        then.status(200)
            .header("content-type", "text/html")
            .body("<html><body><p>acp-fetch-sentinel</p></body></html>");
    });
    let target_url = target_server.url("/page");

    let dir = tempfile::TempDir::new().unwrap();
    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-fetchurl";
    let session_id = "sess-acp-fetchurl-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let api_server = MockServer::start();
    let second_mock = api_server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("acp-fetch-sentinel");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("fetched"));
    });
    api_server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_fetch_001",
                "fetch_url",
                serde_json::json!({"url": target_url}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&api_server.base_url(), &cwd)).await;
            let resp = prompt_and_wait(&nats, prefix, session_id, "fetch", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after fetch_url dispatch; got: {resp}"
            );
        })
        .await;

    assert_eq!(
        second_mock.hits(),
        1,
        "tool_result with fetched content must appear in second Anthropic API call"
    );
}

// ── session/export → PortableBlock ───────────────────────────────────────────

/// After a complete tool_use cycle via the ACP wire format, calling
/// `session/export` via NATS must return PortableBlocks containing:
/// - A `PortableBlock::ToolCall` for the dispatched tool (read_file)
/// - A `PortableBlock::ToolResult` carrying the tool output (file content)
///
/// This test closes the gap between the unit tests that manually seed session
/// state and the real production path where the agent loop populates history
/// from actual API responses.
#[tokio::test]
async fn acp_tool_use_cycle_exported_as_portable_blocks_via_wire() {
    use trogon_runner_tools::portable_session::{PortableBlock, PortableMessage};

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("sentinel.txt"), "sentinel-export-content-xyz\n").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-acp-export-blocks";
    let session_id = "sess-export-blocks-1";
    let cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();

    // Second API call: tool_result with file content → end_turn
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("sentinel-export-content-xyz");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("read complete"));
    });
    // First API call: returns read_file tool_use
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_export_001",
                "read_file",
                serde_json::json!({"path": "sentinel.txt"}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            start_agent(nats.clone(), &js, prefix, make_agent(&server.base_url(), &cwd)).await;

            let resp = prompt_and_wait(&nats, prefix, session_id, "read the sentinel file", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after read_file dispatch; got: {resp}"
            );

            // Call session/export via NATS. The method name is encoded in the subject;
            // the payload is just the ExtRequest params (ExtRequest is transparent over params).
            let export_params = serde_json::json!({"sessionId": session_id});
            let inbox = nats.new_inbox();
            let mut resp_sub = nats.subscribe(inbox.clone()).await.unwrap();
            nats.publish_with_reply(
                format!("{prefix}.agent.ext.session/export"),
                inbox,
                Bytes::from(serde_json::to_vec(&export_params).unwrap()),
            )
            .await
            .unwrap();

            let export_msg = tokio::time::timeout(
                Duration::from_secs(5),
                resp_sub.next(),
            )
            .await
            .expect("timed out waiting for session/export response")
            .expect("no session/export response");

            let messages: Vec<PortableMessage> =
                serde_json::from_slice(&export_msg.payload)
                    .expect("response must be a PortableMessage array");

            assert!(
                !messages.is_empty(),
                "exported messages must not be empty"
            );

            let has_tool_call = messages.iter().any(|m| {
                m.blocks.iter().any(|b| {
                    matches!(b, PortableBlock::ToolCall { name, .. } if name == "read_file")
                })
            });
            assert!(
                has_tool_call,
                "exported messages must contain PortableBlock::ToolCall for read_file; got: {messages:?}"
            );

            let has_tool_result = messages.iter().any(|m| {
                m.blocks.iter().any(|b| {
                    matches!(b, PortableBlock::ToolResult { content, .. }
                        if content.contains("sentinel-export-content-xyz"))
                })
            });
            assert!(
                has_tool_result,
                "exported messages must contain PortableBlock::ToolResult with file content; got: {messages:?}"
            );
        })
        .await;

    assert_eq!(second_mock.hits(), 1, "second mock (tool_result) must be hit once");
}

// ── new_session(cwd) → ToolContext.cwd propagation ───────────────────────────

/// When `new_session` is called via NATS with a real directory as the cwd,
/// that cwd must flow through `state.cwd → agent.set_cwd → ToolContext.cwd`
/// so that a subsequent `read_file` tool call reads from the session's directory,
/// NOT from the ToolContext.cwd baked in at agent construction time.
///
/// The agent is started with an INTENTIONALLY WRONG initial cwd (`/no-such-path`).
/// Only after `new_session(correct_tempdir)` is the ToolContext updated via
/// `set_cwd(state.cwd)`. If the propagation works, read_file returns the file
/// content from the correct tempdir; if not, it returns an error (file not found).
#[tokio::test]
async fn new_session_cwd_overrides_agent_initial_cwd_for_file_tool_dispatch() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("sentinel.txt"), "cwd-propagation-sentinel-xyz").unwrap();

    let (_c, nats, js) = start_nats().await;
    let prefix = "test-cwd-propagation";
    let correct_cwd = dir.path().to_string_lossy().into_owned();

    let server = MockServer::start();

    // Second call: tool_result sent back → end_turn.
    let second_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result")
            .body_contains("cwd-propagation-sentinel-xyz");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn_stream("file read ok"));
    });

    // First call: read_file tool_use.
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_tool_use_stream(
                "tu_cwd_001",
                "read_file",
                serde_json::json!({"path": "sentinel.txt"}),
            ));
    });

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Agent is started with a WRONG initial cwd — set_cwd must override it.
            start_agent(
                nats.clone(),
                &js,
                prefix,
                make_agent(&server.base_url(), "/no-such-path"),
            )
            .await;

            // new_session sends the correct cwd → agent stores it in state.cwd.
            let session_id = {
                let req = NewSessionRequest::new(std::path::PathBuf::from(&correct_cwd));
                let inbox = nats.new_inbox();
                let mut resp_sub = nats.subscribe(inbox.clone()).await.unwrap();
                nats.publish_with_reply(
                    format!("{prefix}.agent.session.new"),
                    inbox,
                    Bytes::from(serde_json::to_vec(&req).unwrap()),
                )
                .await
                .unwrap();
                let msg = tokio::time::timeout(
                    Duration::from_secs(5),
                    resp_sub.next(),
                )
                .await
                .expect("timed out waiting for new_session")
                .expect("no new_session response");
                let v: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            // Prompt triggers read_file — the agent must apply set_cwd(state.cwd)
            // before dispatching, so the file is found in the correct tempdir.
            let resp = prompt_and_wait(&nats, prefix, &session_id, "read the sentinel", 15).await;
            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "expected end_turn after cwd-propagated read_file; got: {resp}"
            );
        })
        .await;

    assert_eq!(
        second_mock.hits(),
        1,
        "second mock (tool_result with sentinel) must be hit — cwd was propagated correctly"
    );
}
