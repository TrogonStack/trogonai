//! Integration tests verifying that the OpenRouter runner correctly dispatches
//! real tool calls (glob, git_status, git_diff, git_log, search_files, fetch_url
//! egress) via its wire format (ToolCallsReady → dispatch_tool → second API call
//! with tool-role message).
//!
//! No Docker needed — uses MockOpenRouterHttpClient and in-memory sessions.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test tool_dispatch_integration \
//!     --features test-helpers

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use agent_client_protocol::{
    Agent as _, ContentBlock, ExtRequest, ForkSessionRequest, NewSessionRequest, PromptRequest,
    SetSessionConfigOptionRequest, SetSessionModelRequest,
};
use trogon_openrouter_runner::{
    AssembledToolCall, MockOpenRouterHttpClient, MockSessionNotifier, OpenRouterAgent,
    OpenRouterEvent,
};
use trogon_tools;

static OR_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn or_env_lock() -> &'static Mutex<()> {
    OR_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_agent(http: Arc<MockOpenRouterHttpClient>) -> OpenRouterAgent<Arc<MockOpenRouterHttpClient>, MockSessionNotifier> {
    OpenRouterAgent::with_deps(MockSessionNotifier::new(), "test-model", "test-key", http)
}

/// Push: first call → tool call, second call → text "done".
fn push_tool_then_done(http: &MockOpenRouterHttpClient, tool_name: &str, args: &str, call_id: &str) {
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: args.to_string(),
        }],
    }]);
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);
}

/// Create a real git repo in `dir` with one commit.
fn init_git_repo(dir: &std::path::Path) {
    Command::new("git").args(["init"]).current_dir(dir).output().unwrap();
    Command::new("git").args(["config", "user.email", "t@t.com"]).current_dir(dir).output().unwrap();
    Command::new("git").args(["config", "user.name", "T"]).current_dir(dir).output().unwrap();
    std::fs::write(dir.join("init.txt"), "init").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(dir).output().unwrap();
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `glob` dispatched via OR wire format — result contains the matching filename.
#[tokio::test]
async fn or_glob_tool_returns_matching_files() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("needle.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("other.txt"), "ignore me").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(&http, "glob", r#"{"pattern":"**/*.rs"}"#, "call_glob_1");

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("glob")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "must have exactly 2 API calls");
            let second = &calls[1];
            let tool_msg = second.messages.iter().find(|m| m.role == "tool")
                .expect("second API call must include a tool-role message with tool result");
            assert!(
                tool_msg.content.contains("needle.rs"),
                "glob result must contain 'needle.rs'; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `git_status` dispatched via OR wire format — result is git output.
#[tokio::test]
async fn or_git_status_tool_returns_output() {
    let dir = tempfile::TempDir::new().unwrap();
    init_git_repo(dir.path());

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(&http, "git_status", r#"{}"#, "call_gitstatus_1");

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("git status")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool result");
            assert!(
                !tool_msg.content.is_empty(),
                "git_status result must be non-empty"
            );
            // Must not be "Unknown tool" — confirms dispatch reached real tool
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "git_status must be dispatched; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `git_diff` dispatched via OR wire format — result is diff output (or "nothing").
#[tokio::test]
async fn or_git_diff_tool_returns_output() {
    let dir = tempfile::TempDir::new().unwrap();
    init_git_repo(dir.path());
    // Modify a file so there's an unstaged diff.
    std::fs::write(dir.path().join("init.txt"), "modified content").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(&http, "git_diff", r#"{}"#, "call_gitdiff_1");

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("git diff")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool result");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "git_diff must be dispatched; got: {}",
                tool_msg.content
            );
            // A diff of the modified file must appear
            assert!(
                tool_msg.content.contains("init.txt") || tool_msg.content.contains("diff"),
                "git_diff result must contain diff output; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `git_log` dispatched via OR wire format — result contains the initial commit.
#[tokio::test]
async fn or_git_log_tool_returns_commits() {
    let dir = tempfile::TempDir::new().unwrap();
    init_git_repo(dir.path());

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(&http, "git_log", r#"{}"#, "call_gitlog_1");

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("git log")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool result");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "git_log must be dispatched; got: {}",
                tool_msg.content
            );
            assert!(
                tool_msg.content.contains("initial"),
                "git_log must include the initial commit; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `search_files` dispatched via OR wire format — result contains matched line.
#[tokio::test]
async fn or_search_files_tool_returns_matches() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("haystack.txt"), "the quick brown fox\njumps over the lazy dog\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "search_files",
        r#"{"pattern":"quick brown","path":"."}"#,
        "call_search_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("search")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool result");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "search_files must be dispatched; got: {}",
                tool_msg.content
            );
            assert!(
                tool_msg.content.contains("haystack.txt") || tool_msg.content.contains("quick brown"),
                "search_files result must contain match; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `fetch_url` with a link-local address is blocked by the OR egress policy.
#[tokio::test]
async fn or_fetch_url_link_local_blocked_by_egress_policy() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "fetch_url",
        r#"{"url":"http://169.254.169.254/latest/meta-data/"}"#,
        "call_fetch_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("fetch")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool-role message");
            assert!(
                tool_msg.content.contains("blocked by egress policy"),
                "fetch_url for link-local address must be blocked; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `todo_write` dispatched via OR wire format — result is "OK".
#[tokio::test]
async fn or_todo_write_dispatched_returns_ok() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "todo_write",
        r#"{"id":"t1","content":"implement tests","status":"pending"}"#,
        "call_todo_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("add todo")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool result");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "todo_write must be dispatched; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// After cross-runner session/import, the destination runner uses the cwd
/// that was passed to new_session (not the source session's cwd).
/// This verifies the "cross-runner then file tool in destination" flow.
#[tokio::test]
async fn or_file_tool_after_import_uses_new_session_cwd() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("target.txt"), "cross-runner-content").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    // First call: read_file tool call
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: "call_rf_1".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"target.txt"}"#.to_string(),
        }],
    }]);
    // Second call: done
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            // 1. Create session with destination cwd
            let new_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = new_resp.session_id.clone();

            // 2. Import history (simulates cross-runner switch arriving with prior messages)
            let import_params = serde_json::value::RawValue::from_string(format!(
                r#"{{"sessionId":"{}","messages":[{{"role":"user","text":"prior turn"}}]}}"#,
                sid
            ))
            .unwrap();
            agent
                .ext_method(ExtRequest::new("session/import", import_params.into()))
                .await
                .expect("session/import must succeed");

            // 3. Prompt — triggers read_file tool call
            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("read target.txt")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second call must have tool result");
            assert!(
                tool_msg.content.contains("cross-runner-content"),
                "read_file must read from destination cwd; got: {}",
                tool_msg.content
            );
        })
        .await;
}

// ── _meta.systemPrompt → OR wire ─────────────────────────────────────────────

/// `_meta.systemPrompt` passed in `new_session` must appear as a system-role
/// message in the first API call wire.
#[tokio::test]
async fn or_meta_system_prompt_sent_to_api_wire() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let mut meta = serde_json::Map::new();
            meta.insert("systemPrompt".to_string(), serde_json::json!("always respond in JSON"));
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")).meta(meta))
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sess.session_id, vec![ContentBlock::from("hi")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            assert!(!calls.is_empty());
            let system_msg = calls[0]
                .messages
                .iter()
                .find(|m| m.role == "system")
                .expect("_meta.systemPrompt must appear as system-role message in OR wire");
            assert!(
                system_msg.content.contains("always respond in JSON"),
                "system message must contain the _meta.systemPrompt value; got: {}",
                system_msg.content
            );
        })
        .await;
}

// ── OR set_model appears in wire ──────────────────────────────────────────────

/// After `set_session_model`, the next prompt must use the new model in the
/// OpenRouter API call.
#[tokio::test]
async fn or_set_model_appears_in_wire_request() {
    let _guard = or_env_lock().lock().unwrap();
    unsafe {
        std::env::set_var("OPENROUTER_MODELS", "test-model:Test Model,alt-model:Alt Model");
    }
    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
    let agent = make_agent(Arc::clone(&http));
    unsafe {
        std::env::remove_var("OPENROUTER_MODELS");
    }

    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let sid = sess.session_id;

            agent
                .set_session_model(SetSessionModelRequest::new(sid.clone(), "alt-model"))
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("q")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].model, "alt-model",
                "set_session_model must change the model in OR wire request; got: {}",
                calls[0].model
            );
        })
        .await;
}

// ── OR enabled_tools filter appears in wire tools array ───────────────────────

/// Disabling a tool via `set_session_config_option` must remove it from the
/// `tools` array sent to the OpenRouter API.
#[tokio::test]
async fn or_disabled_tool_absent_from_wire_tools_array() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let sid = sess.session_id;

            // Disable glob tool
            agent
                .set_session_config_option(SetSessionConfigOptionRequest::new(
                    sid.clone(),
                    "glob",
                    "disabled",
                ))
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("search files")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            assert!(!calls.is_empty());
            let tool_names: Vec<&str> = calls[0].tools.iter().map(|t| t.name.as_str()).collect();
            assert!(
                !tool_names.contains(&"glob"),
                "disabled tool 'glob' must not appear in OR wire tools array; got: {tool_names:?}"
            );
        })
        .await;
}

// ── TROGON.md → OR system prompt wire ────────────────────────────────────────

/// A TROGON.md file in the session cwd must be injected into the system prompt
/// sent to the OpenRouter API.
#[tokio::test]
async fn or_trogon_md_injected_into_system_prompt_in_wire_request() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "or-project-rules: keep it short").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sess.session_id, vec![ContentBlock::from("hi")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let system_msg = calls[0]
                .messages
                .iter()
                .find(|m| m.role == "system")
                .expect("TROGON.md must appear as system message in OR wire");
            assert!(
                system_msg.content.contains("or-project-rules"),
                "system message must contain TROGON.md content; got: {}",
                system_msg.content
            );
        })
        .await;
}

// ── OR all tools present in wire request by default ───────────────────────────

/// A freshly created session must forward all trogon tools to the OR API.
#[tokio::test]
async fn or_all_tools_present_in_wire_request_by_default() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sess.session_id, vec![ContentBlock::from("hi")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let expected = trogon_tools::all_tool_defs().len();
            let got = calls[0].tools.len();
            assert_eq!(
                got, expected,
                "new session must include all {expected} trogon tools in wire request; got {got}"
            );
        })
        .await;
}

// ── OR _meta.systemPrompt + TROGON.md merged into one system message ──────────

/// When both TROGON.md and `_meta.systemPrompt` are provided, both must appear
/// in the single system message sent to the OpenRouter API.
#[tokio::test]
async fn or_meta_system_prompt_and_trogon_md_both_in_system_message() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "or-md-rules: prefer brevity").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let mut meta = serde_json::Map::new();
            meta.insert("systemPrompt".to_string(), serde_json::json!("or-meta-rules: use JSON"));
            let sess = agent
                .new_session(
                    NewSessionRequest::new(PathBuf::from(dir.path())).meta(meta),
                )
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sess.session_id, vec![ContentBlock::from("hello")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let system_msg = calls[0]
                .messages
                .iter()
                .find(|m| m.role == "system")
                .expect("system message must be present when both TROGON.md and _meta.systemPrompt are set");
            assert!(
                system_msg.content.contains("or-md-rules"),
                "system message must contain TROGON.md content; got: {}",
                system_msg.content
            );
            assert!(
                system_msg.content.contains("or-meta-rules"),
                "system message must contain _meta.systemPrompt content; got: {}",
                system_msg.content
            );
        })
        .await;
}

// ── notebook_edit → OR wire ───────────────────────────────────────────────────

/// `notebook_edit` dispatched via OR wire format — the tool result is sent in
/// the second OpenRouter API call and the notebook cell is updated on disk.
#[tokio::test]
async fn or_notebook_edit_tool_dispatched_via_wire_format() {
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

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "notebook_edit",
        r#"{"path":"nb.ipynb","cell_index":0,"content":"updated content"}"#,
        "call_nbedit_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("edit notebook")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "must have exactly 2 API calls");
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second API call must include a tool-role message");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "notebook_edit must be dispatched; got: {}",
                tool_msg.content
            );

            let raw = std::fs::read_to_string(dir.path().join("nb.ipynb")).unwrap();
            let nb: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let source = nb["cells"][0]["source"].to_string();
            assert!(
                source.contains("updated content"),
                "notebook cell must be updated on disk; got: {source}"
            );
        })
        .await;
}

// ── write_file → OR wire ─────────────────────────────────────────────────────

/// `write_file` dispatched via OR wire format — file is created on disk and
/// the tool result is sent in the second OpenRouter API call.
#[tokio::test]
async fn or_write_file_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "write_file",
        r#"{"path":"output.rs","content":"fn main() {}\n"}"#,
        "call_wf_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("write file")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "must have exactly 2 API calls");
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second API call must include a tool-role message");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "write_file must be dispatched; got: {}",
                tool_msg.content
            );

            let on_disk = std::fs::read_to_string(dir.path().join("output.rs"))
                .expect("write_file must create output.rs on disk");
            assert!(
                on_disk.contains("fn main()"),
                "file content must match; got: {on_disk}"
            );
        })
        .await;
}

// ── str_replace → OR wire ─────────────────────────────────────────────────────

/// `str_replace` dispatched via OR wire format — edit is applied on disk and
/// the tool result is sent in the second OpenRouter API call.
#[tokio::test]
async fn or_str_replace_tool_dispatched_via_wire_format() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn old_impl() {}\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "str_replace",
        r#"{"path":"code.rs","old_str":"fn old_impl()","new_str":"fn new_impl()"}"#,
        "call_str_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            let result = agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("replace fn")]))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "expected end_turn: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "must have exactly 2 API calls");
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool")
                .expect("second API call must include a tool-role message");
            assert!(
                !tool_msg.content.contains("Unknown tool"),
                "str_replace must be dispatched; got: {}",
                tool_msg.content
            );
            assert!(
                !tool_msg.content.starts_with("Error"),
                "str_replace must succeed; got: {}",
                tool_msg.content
            );

            let on_disk = std::fs::read_to_string(dir.path().join("code.rs"))
                .expect("code.rs must still exist after str_replace");
            assert!(
                on_disk.contains("fn new_impl()"),
                "file must contain replacement; got: {on_disk}"
            );
            assert!(
                !on_disk.contains("fn old_impl()"),
                "file must not contain old text; got: {on_disk}"
            );
        })
        .await;
}

// ── OR fork inherits disabled tool excluded from wire ─────────────────────────

/// A forked session must inherit the parent's disabled tools and exclude them
/// from the tools array sent to the OpenRouter API.
#[tokio::test]
async fn or_fork_inherits_disabled_tool_excluded_from_wire() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "fork ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let parent = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let parent_id = parent.session_id;

            agent
                .set_session_config_option(SetSessionConfigOptionRequest::new(
                    parent_id.clone(),
                    "glob",
                    "disabled",
                ))
                .await
                .unwrap();

            let fork = agent
                .fork_session(ForkSessionRequest::new(parent_id, PathBuf::from("/tmp")))
                .await
                .unwrap();
            let fork_id = fork.session_id;

            agent
                .prompt(PromptRequest::new(fork_id, vec![ContentBlock::from("hi")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let tool_names: Vec<&str> = calls[0].tools.iter().map(|t| t.name.as_str()).collect();
            assert!(
                !tool_names.contains(&"glob"),
                "fork must inherit disabled 'glob' from parent; got tools: {tool_names:?}"
            );
        })
        .await;
}

// ── remaining tool wire-format tests ─────────────────────────────────────────

/// `read_file` dispatched via OR wire format — `tool`-role message in the second
/// API call contains the file's content.
#[tokio::test]
async fn or_read_file_tool_returns_content() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("data.txt"), "or-read-sentinel-abc123").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(&http, "read_file", r#"{"path":"data.txt"}"#, "call_rf_1");

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            let sid = resp.session_id;

            agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("read")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "must have exactly 2 API calls");
            let tool_msg = calls[1]
                .messages
                .iter()
                .find(|m| m.role == "tool")
                .expect("second call must include a tool-role message");
            assert!(
                tool_msg.content.contains("or-read-sentinel-abc123"),
                "read_file result must contain file content; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `write_file` dispatched via OR wire format — file is created on disk and the
/// tool-role message confirms success.
#[tokio::test]
async fn or_write_file_tool_creates_file_on_disk() {
    let dir = tempfile::TempDir::new().unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "write_file",
        r#"{"path":"out.txt","content":"or-write-sentinel-999"}"#,
        "call_wf_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(resp.session_id, vec![ContentBlock::from("write")]))
                .await
                .unwrap();

            let written = std::fs::read_to_string(dir.path().join("out.txt"))
                .expect("write_file must create the file");
            assert!(
                written.contains("or-write-sentinel-999"),
                "written content must match; got: {written}"
            );
            let calls = http.calls.lock().unwrap();
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool").expect("tool message must exist");
            assert!(!tool_msg.content.contains("Unknown tool"), "write_file must be dispatched; got: {}", tool_msg.content);
        })
        .await;
}

/// `list_dir` dispatched via OR wire format — result contains filenames from cwd.
#[tokio::test]
async fn or_list_dir_tool_returns_directory_listing() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("beta.rs"), "fn b(){}").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(&http, "list_dir", r#"{}"#, "call_ld_1");

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(resp.session_id, vec![ContentBlock::from("list")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool").expect("tool message");
            assert!(
                tool_msg.content.contains("beta.rs"),
                "list_dir must include 'beta.rs'; got: {}",
                tool_msg.content
            );
        })
        .await;
}

/// `str_replace` dispatched via OR wire format — modifies the file on disk.
#[tokio::test]
async fn or_str_replace_tool_modifies_file_on_disk() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("edit.rs"), "fn original() {}").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "str_replace",
        r#"{"path":"edit.rs","old_str":"original","new_str":"replaced"}"#,
        "call_sr_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(resp.session_id, vec![ContentBlock::from("replace")]))
                .await
                .unwrap();

            let content = std::fs::read_to_string(dir.path().join("edit.rs")).unwrap();
            assert!(content.contains("replaced"), "str_replace must have applied; got: {content}");
            let calls = http.calls.lock().unwrap();
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool").expect("tool message");
            assert!(!tool_msg.content.contains("Unknown tool"), "str_replace must be dispatched; got: {}", tool_msg.content);
        })
        .await;
}

// ── session/list_children stub ────────────────────────────────────────────────

/// `session/list_children` on the OpenRouter runner is a stub that always
/// returns an empty JSON array.  This test verifies the endpoint is registered
/// and does not error, consistent with the programming.md spec for the runner.
#[tokio::test]
async fn or_ext_method_list_children_returns_empty_array() {
    let http = Arc::new(MockOpenRouterHttpClient::new());
    let agent = make_agent(Arc::clone(&http));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let sid = sess.session_id;

            let params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": sid }).to_string(),
                )
                .unwrap()
                .into();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", params))
                .await
                .expect("session/list_children must not error on OpenRouter runner");

            let val: serde_json::Value =
                serde_json::from_str(resp.0.get()).expect("response must be valid JSON");
            assert!(
                val.is_array(),
                "session/list_children must return a JSON array; got: {val}"
            );
            assert_eq!(
                val.as_array().unwrap().len(),
                0,
                "OpenRouter session/list_children stub must return empty array; got: {val}"
            );
        })
        .await;
}

/// `notebook_edit` dispatched via OR wire format — updates the cell source in
/// a real `.ipynb` file on disk.
#[tokio::test]
async fn or_notebook_edit_tool_updates_cell_on_disk() {
    let dir = tempfile::TempDir::new().unwrap();
    let nb = serde_json::json!({
        "nbformat": 4, "nbformat_minor": 5, "metadata": {},
        "cells": [{"cell_type":"code","source":"print('old')","metadata":{},"outputs":[],"execution_count":null}]
    });
    std::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string(&nb).unwrap()).unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_then_done(
        &http,
        "notebook_edit",
        r#"{"path":"nb.ipynb","cell_index":0,"content":"print('new')"}"#,
        "call_ne_1",
    );

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(resp.session_id, vec![ContentBlock::from("edit nb")]))
                .await
                .unwrap();

            let nb_after: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.path().join("nb.ipynb")).unwrap(),
            )
            .unwrap();
            let source = &nb_after["cells"][0]["source"];
            let src = if let Some(s) = source.as_str() {
                s.to_string()
            } else if let Some(arr) = source.as_array() {
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("")
            } else {
                String::new()
            };
            assert!(src.contains("new"), "notebook_edit must update cell; got: {src}");
            let calls = http.calls.lock().unwrap();
            let tool_msg = calls[1].messages.iter().find(|m| m.role == "tool").expect("tool message");
            assert!(!tool_msg.content.contains("Unknown tool"), "notebook_edit must be dispatched; got: {}", tool_msg.content);
        })
        .await;
}

// ── TROGON.md absent ─────────────────────────────────────────────────────────

/// When a session is started in a directory that has no `TROGON.md`, the
/// OR runner must not crash and must complete prompts normally with `EndTurn`.
///
/// Exercises the real scenario where users open a project that has no
/// TROGON.md — previously untested for openrouter.
#[tokio::test]
async fn or_no_crash_when_trogon_md_absent_from_session_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    // Intentionally NOT writing TROGON.md in dir.

    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();

            let result = agent
                .prompt(PromptRequest::new(
                    sess.session_id,
                    vec![ContentBlock::from("hello")],
                ))
                .await
                .expect("prompt must not error when TROGON.md is absent");

            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "prompt must complete with EndTurn when TROGON.md is absent; got: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            if let Some(sys) = calls[0].messages.iter().find(|m| m.role == "system") {
                assert!(
                    !sys.content.contains("TROGON"),
                    "system message must not inject TROGON.md content when file is absent; got: {}",
                    sys.content
                );
            }
        })
        .await;
}

// ── Multi-tool programming chain ──────────────────────────────────────────────

/// Full OR programming tool chain: `read_file` → `str_replace` → `git_diff`
/// → `end_turn`, verifying that the OR runner correctly routes three
/// sequential tool calls in a single session without losing state.
///
/// Exercises the realistic multi-step code-editing flow that is the primary
/// use-case for OpenRouter programming mode.
#[tokio::test]
async fn or_programming_tool_chain_read_str_replace_git_diff() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("src.rs"), "fn old_function() {}\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());

    // Call 1: read_file tool_use.
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: "c-rf".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"src.rs"}"#.to_string(),
        }],
    }]);
    // Call 2: str_replace tool_use (LLM saw "old_function" in tool result).
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: "c-sr".to_string(),
            name: "str_replace".to_string(),
            arguments: r#"{"path":"src.rs","old_str":"old_function","new_str":"new_function"}"#
                .to_string(),
        }],
    }]);
    // Call 3: git_diff tool_use (LLM wants to inspect the diff).
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: "c-gd".to_string(),
            name: "git_diff".to_string(),
            arguments: "{}".to_string(),
        }],
    }]);
    // Call 4: LLM is satisfied → end_turn.
    http.push_response(vec![OpenRouterEvent::TextDelta {
        text: "done with edits".to_string(),
    }]);

    let agent = make_agent(Arc::clone(&http));
    tokio::task::LocalSet::new()
        .run_until(async move {
            let sess = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap();

            let result = agent
                .prompt(PromptRequest::new(
                    sess.session_id,
                    vec![ContentBlock::from("refactor the function")],
                ))
                .await
                .unwrap();

            assert!(
                matches!(result.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "programming chain must complete with EndTurn; got: {:?}",
                result.stop_reason
            );

            let calls = http.calls.lock().unwrap();
            assert_eq!(
                calls.len(),
                4,
                "must have exactly 4 API calls (read_file + str_replace + git_diff + done); got: {}",
                calls.len()
            );

            let on_disk = std::fs::read_to_string(dir.path().join("src.rs")).unwrap();
            assert!(
                on_disk.contains("new_function"),
                "str_replace must have renamed old_function → new_function; got: {on_disk:?}"
            );
            assert!(
                !on_disk.contains("old_function"),
                "old_function must be gone after str_replace; got: {on_disk:?}"
            );
        })
        .await;
}
