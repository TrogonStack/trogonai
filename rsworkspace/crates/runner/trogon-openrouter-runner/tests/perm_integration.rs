#![allow(clippy::await_holding_lock)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for granular permissions in OpenRouterAgent.
//!
//! Tests verify the full pipeline end-to-end:
//!   TROGON.md (real FS) → parse rules → check per tool call → allow/deny → audit log
//!
//! No Docker needed — uses MockOpenRouterHttpClient and a real tempdir for TROGON.md.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test perm_integration --features test-helpers

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use acp_nats::AgentHandler;
use agent_client_protocol::schema::v1::{
    ContentBlock, NewSessionRequest, PromptRequest, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionModeRequest, ToolCallStatus,
};
use trogon_openrouter_runner::{
    AssembledToolCall, MockOpenRouterHttpClient, MockSessionNotifier, OpenRouterAgent, OpenRouterEvent,
};
use trogon_runner_tools::session_store::AuditOutcome;

// ── env-var lock ──────────────────────────────────────────────────────────────

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ── helpers ───────────────────────────────────────────────────────────────────

type TestAgent = OpenRouterAgent<Arc<MockOpenRouterHttpClient>, MockSessionNotifier>;

fn make_agent(http: Arc<MockOpenRouterHttpClient>) -> TestAgent {
    unsafe {
        std::env::remove_var("OPENROUTER_MODELS");
        std::env::remove_var("OPENROUTER_PROMPT_TIMEOUT_SECS");
    }
    OpenRouterAgent::with_deps(MockSessionNotifier::new(), "test-model", "test-key", http)
}

fn local() -> tokio::task::LocalSet {
    tokio::task::LocalSet::new()
}

fn done_events() -> Vec<OpenRouterEvent> {
    vec![OpenRouterEvent::Done]
}

fn push_tool_call(http: &MockOpenRouterHttpClient, call_id: &str, name: &str, args: &str) {
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: call_id.to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }],
    }]);
}

fn second_call_has_denied_output(http: &MockOpenRouterHttpClient, call_id: &str) -> bool {
    let calls = http.calls.lock().unwrap();
    calls
        .get(1)
        .map(|call| {
            call.messages
                .iter()
                .any(|m| m.tool_call_id.as_deref() == Some(call_id) && m.content.contains("permission denied"))
        })
        .unwrap_or(false)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// deny_paths in TROGON.md blocks the matching file tool and records Denied in audit.
#[tokio::test]
async fn deny_path_from_trogon_md_blocks_file_tool() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_call(&http, "cid-deny", "read_file", r#"{"path":".env"}"#);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap()
                .session_id
                .to_string();

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")]))
                .await
                .unwrap();

            assert!(
                second_call_has_denied_output(&http, "cid-deny"),
                "read_file on .env must produce 'permission denied' in follow-up HTTP call"
            );

            let audit = agent.test_session_audit_log(&sid).await;
            assert_eq!(audit.len(), 1, "one tool call must produce one audit entry");
            assert_eq!(audit[0].tool, "read_file");
            assert_eq!(
                audit[0].outcome,
                AuditOutcome::Denied,
                "audit must record Denied for .env"
            );
        })
        .await;
}

/// bypassPermissions flag in new_session meta skips deny_paths from TROGON.md.
#[tokio::test]
async fn bypass_permissions_meta_skips_deny_rule_from_trogon_md() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_call(&http, "cid-bypass", "read_file", r#"{"path":".env"}"#);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let mut meta = serde_json::Map::new();
            meta.insert("bypassPermissions".to_string(), serde_json::json!(true));
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())).meta(meta))
                .await
                .unwrap()
                .session_id
                .to_string();

            assert_eq!(
                agent.test_session_mode(&sid).await.as_deref(),
                Some("bypassPermissions"),
                "session mode must be bypassPermissions after new_session with meta flag"
            );

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let has_denied = calls
                .get(1)
                .map(|call| {
                    call.messages
                        .iter()
                        .any(|m| m.tool_call_id.is_some() && m.content.contains("permission denied"))
                })
                .unwrap_or(false);
            drop(calls);
            assert!(
                !has_denied,
                "bypass mode must not block tools even when deny_paths matches"
            );

            let audit = agent.test_session_audit_log(&sid).await;
            assert_eq!(audit.len(), 1);
            assert_eq!(
                audit[0].outcome,
                AuditOutcome::Allowed,
                "bypass mode must record Allowed in audit"
            );
        })
        .await;
}

/// set_session_mode bypassPermissions after session creation overrides deny rule from TROGON.md.
#[tokio::test]
async fn set_session_mode_bypass_overrides_deny_rule() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_call(&http, "cid-mode", "read_file", r#"{"path":".env"}"#);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap()
                .session_id
                .to_string();

            agent
                .set_session_mode(SetSessionModeRequest::new(sid.clone(), "bypassPermissions"))
                .await
                .unwrap();

            assert_eq!(
                agent.test_session_mode(&sid).await.as_deref(),
                Some("bypassPermissions"),
                "session mode must update to bypassPermissions"
            );

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")]))
                .await
                .unwrap();

            let calls = http.calls.lock().unwrap();
            let has_denied = calls
                .get(1)
                .map(|call| {
                    call.messages
                        .iter()
                        .any(|m| m.tool_call_id.is_some() && m.content.contains("permission denied"))
                })
                .unwrap_or(false);
            drop(calls);
            assert!(
                !has_denied,
                "bypass mode set via set_session_mode must allow tools denied by TROGON.md"
            );

            let audit = agent.test_session_audit_log(&sid).await;
            assert_eq!(audit.len(), 1);
            assert_eq!(
                audit[0].outcome,
                AuditOutcome::Allowed,
                "audit must record Allowed after set_session_mode bypass"
            );
        })
        .await;
}

/// deny_commands in TROGON.md blocks a bash command with matching prefix.
#[tokio::test]
async fn deny_command_from_trogon_md_blocks_bash() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_commands: rm -rf\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_call(&http, "cid-bash", "bash", r#"{"command":"rm -rf /"}"#);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap()
                .session_id
                .to_string();

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("run")]))
                .await
                .unwrap();

            assert!(
                second_call_has_denied_output(&http, "cid-bash"),
                "bash 'rm -rf /' must be blocked by deny_commands: rm -rf"
            );

            let audit = agent.test_session_audit_log(&sid).await;
            assert_eq!(audit.len(), 1);
            assert_eq!(audit[0].tool, "bash");
            assert_eq!(
                audit[0].outcome,
                AuditOutcome::Denied,
                "audit must record Denied for denied bash command"
            );
        })
        .await;
}

/// Permission rules injected via set_session_config_option("permissions", ...) deny the tool.
#[tokio::test]
async fn permission_rules_via_config_option_denies_tool() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    // TROGON.md has no permission rules — deny comes from config option only
    std::fs::write(dir.path().join("TROGON.md"), "# Project\nNo rules here.\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_call(&http, "cid-cfg", "read_file", r#"{"path":".env"}"#);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap()
                .session_id
                .to_string();

            agent
                .set_session_config_option(SetSessionConfigOptionRequest::new(
                    sid.clone(),
                    "permissions",
                    "## Permissions\ndeny_paths: .env\n",
                ))
                .await
                .unwrap();

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")]))
                .await
                .unwrap();

            assert!(
                second_call_has_denied_output(&http, "cid-cfg"),
                "deny rule injected via set_session_config_option must block matching file tool"
            );

            let audit = agent.test_session_audit_log(&sid).await;
            assert_eq!(audit.len(), 1);
            assert_eq!(
                audit[0].outcome,
                AuditOutcome::Denied,
                "audit must record Denied when rule comes from config option"
            );
        })
        .await;
}

/// Multiple tool calls in one prompt: one Allowed (README.md), one Denied (secrets/).
#[tokio::test]
async fn mixed_allowed_and_denied_tools_in_same_prompt() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: secrets/**\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    http.push_response(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![
            AssembledToolCall {
                id: "cid-ok".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"README.md"}"#.to_string(),
            },
            AssembledToolCall {
                id: "cid-bad".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"secrets/key.pem"}"#.to_string(),
            },
        ],
    }]);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap()
                .session_id
                .to_string();

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read both")]))
                .await
                .unwrap();

            let audit = agent.test_session_audit_log(&sid).await;
            assert_eq!(audit.len(), 2, "audit must have one entry per tool call");

            let has_allowed = audit.iter().any(|e| e.outcome == AuditOutcome::Allowed);
            let has_denied = audit.iter().any(|e| e.outcome == AuditOutcome::Denied);
            assert!(has_allowed, "audit must contain Allowed entry for README.md");
            assert!(has_denied, "audit must contain Denied entry for secrets/key.pem");

            let denied = audit.iter().find(|e| e.outcome == AuditOutcome::Denied).unwrap();
            assert!(
                denied.input_summary.contains("secrets"),
                "denied entry must reference the secrets path; got: {}",
                denied.input_summary
            );

            let calls = http.calls.lock().unwrap();
            let second = calls.get(1).expect("must make a second HTTP call after tool dispatch");
            let denied_msg = second
                .messages
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some("cid-bad"));
            assert!(denied_msg.is_some(), "second HTTP call must include result for cid-bad");
            assert!(
                denied_msg.unwrap().content.contains("permission denied"),
                "denied tool result must contain 'permission denied'; got: {}",
                denied_msg.unwrap().content
            );
        })
        .await;
}

/// A denied tool must NOT get ToolCall(InProgress); only Pending then ToolCallUpdate(Completed).
#[tokio::test]
async fn denied_tool_has_no_inprogress_notification() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let http = Arc::new(MockOpenRouterHttpClient::new());
    push_tool_call(&http, "cid-denied", "read_file", r#"{"path":".env"}"#);
    http.push_response(done_events());
    let agent = make_agent(Arc::clone(&http));

    local()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from(dir.path())))
                .await
                .unwrap()
                .session_id
                .to_string();

            agent
                .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")]))
                .await
                .unwrap();

            let notifs = agent.test_notifier().notifications.lock().unwrap();

            // OpenRouter dispatches from ToolCallsReady (no Pending stage from stream parsing).
            // InProgress must NOT be sent for a denied tool.
            let has_inprogress = notifs.iter().any(|n| {
                matches!(&n.update, SessionUpdate::ToolCall(tc)
                if tc.tool_call_id.to_string() == "cid-denied"
                    && tc.status == ToolCallStatus::InProgress)
            });
            assert!(
                !has_inprogress,
                "ToolCall(InProgress) must NOT be sent for a denied tool; got: {notifs:?}"
            );

            // ToolCallUpdate(Completed) with "permission denied" must be sent
            let has_denied_update = notifs.iter().any(|n| {
                if let SessionUpdate::ToolCallUpdate(tcu) = &n.update {
                    tcu.tool_call_id.to_string() == "cid-denied"
                        && tcu
                            .fields
                            .raw_output
                            .as_ref()
                            .and_then(|v| v.as_str())
                            .map(|s| s.contains("permission denied"))
                            .unwrap_or(false)
                } else {
                    false
                }
            });
            assert!(
                has_denied_update,
                "ToolCallUpdate with 'permission denied' must be sent after denial; got: {notifs:?}"
            );
        })
        .await;
}
