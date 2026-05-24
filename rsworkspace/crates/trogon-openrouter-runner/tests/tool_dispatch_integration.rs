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
