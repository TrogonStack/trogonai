#![allow(clippy::await_holding_lock)]
//! Integration tests for granular permissions in XaiAgent.
//!
//! Tests verify the full pipeline end-to-end:
//!   TROGON.md (real FS) → parse rules → check per tool call → allow/deny → audit log
//!
//! No Docker needed — uses MockXaiHttpClient and a real tempdir for TROGON.md.
//!
//! Run with:
//!   cargo test -p trogon-xai-runner --test perm_integration --features test-helpers

use std::sync::{Arc, Mutex, OnceLock};

use agent_client_protocol::{
    Agent as _, ContentBlock, NewSessionRequest, PromptRequest,
    SetSessionConfigOptionRequest, SetSessionModeRequest, SessionUpdate, ToolCallStatus,
};
use trogon_runner_tools::session_store::AuditOutcome;
use trogon_xai_runner::{InputItem, MockSessionNotifier, MockXaiHttpClient, XaiAgent, XaiEvent};

// ── env-var lock (shared with other integration tests) ───────────────────────

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ── helpers ───────────────────────────────────────────────────────────────────

type TestAgent = XaiAgent<Arc<MockXaiHttpClient>, MockSessionNotifier>;

async fn make_agent(mock: Arc<MockXaiHttpClient>) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "fake-key", mock)
}

fn done_response() -> Vec<XaiEvent> {
    vec![XaiEvent::Done]
}

fn push_function_call(mock: &MockXaiHttpClient, call_id: &str, name: &str, args: &str) {
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "r1".to_string() },
        XaiEvent::FunctionCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        },
        XaiEvent::Done,
    ]);
}

fn second_call_has_denied_output(mock: &MockXaiHttpClient, call_id: &str) -> bool {
    let calls = mock.calls.lock().unwrap();
    calls.get(1).map(|call| {
        call.input.iter().any(|item| {
            matches!(item, InputItem::FunctionCallOutput { call_id: cid, output }
                if cid == call_id && output.contains("permission denied"))
        })
    }).unwrap_or(false)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// deny_paths in TROGON.md blocks the matching file tool and records Denied in audit.
#[tokio::test]
async fn deny_path_from_trogon_md_blocks_file_tool() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("TROGON.md"),
        "## Permissions\ndeny_paths: .env\n",
    ).unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    push_function_call(&mock, "cid-deny", "read_file", r#"{"path":".env"}"#);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await.unwrap().session_id.to_string();

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")])).await.unwrap();

    assert!(
        second_call_has_denied_output(&mock, "cid-deny"),
        "read_file on .env must produce 'permission denied' in follow-up HTTP call"
    );

    let audit = agent.test_session_audit_log(&sid).await;
    assert_eq!(audit.len(), 1, "one tool call must produce one audit entry");
    assert_eq!(audit[0].tool, "read_file");
    assert_eq!(audit[0].outcome, AuditOutcome::Denied, "audit must record Denied for .env");
}

/// bypassPermissions flag in new_session meta skips deny_paths from TROGON.md.
#[tokio::test]
async fn bypass_permissions_meta_skips_deny_rule_from_trogon_md() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("TROGON.md"),
        "## Permissions\ndeny_paths: .env\n",
    ).unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    push_function_call(&mock, "cid-bypass", "read_file", r#"{"path":".env"}"#);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let mut meta = serde_json::Map::new();
    meta.insert("bypassPermissions".to_string(), serde_json::json!(true));
    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()).meta(meta))
        .await.unwrap().session_id.to_string();

    assert_eq!(
        agent.test_session_mode(&sid).await.as_deref(),
        Some("bypassPermissions"),
        "session mode must be bypassPermissions after new_session with meta flag"
    );

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")])).await.unwrap();

    let calls = mock.calls.lock().unwrap();
    let has_denied = calls.get(1).map(|call| {
        call.input.iter().any(|item| {
            matches!(item, InputItem::FunctionCallOutput { output, .. } if output.contains("permission denied"))
        })
    }).unwrap_or(false);
    drop(calls);
    assert!(!has_denied, "bypass mode must not block tools even when deny_paths matches");

    let audit = agent.test_session_audit_log(&sid).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].outcome, AuditOutcome::Allowed,
        "bypass mode must record Allowed in audit"
    );
}

/// set_session_mode bypassPermissions after session creation overrides deny rule from TROGON.md.
#[tokio::test]
async fn set_session_mode_bypass_overrides_deny_rule() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("TROGON.md"),
        "## Permissions\ndeny_paths: .env\n",
    ).unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    push_function_call(&mock, "cid-mode", "read_file", r#"{"path":".env"}"#);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await.unwrap().session_id.to_string();

    agent
        .set_session_mode(SetSessionModeRequest::new(sid.clone(), "bypassPermissions"))
        .await.unwrap();

    assert_eq!(
        agent.test_session_mode(&sid).await.as_deref(),
        Some("bypassPermissions"),
        "session mode must update to bypassPermissions"
    );

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")])).await.unwrap();

    let calls = mock.calls.lock().unwrap();
    let has_denied = calls.get(1).map(|call| {
        call.input.iter().any(|item| {
            matches!(item, InputItem::FunctionCallOutput { output, .. } if output.contains("permission denied"))
        })
    }).unwrap_or(false);
    drop(calls);
    assert!(!has_denied, "bypass mode set via set_session_mode must allow tools denied by TROGON.md");

    let audit = agent.test_session_audit_log(&sid).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].outcome, AuditOutcome::Allowed,
        "audit must record Allowed after set_session_mode bypass"
    );
}

/// deny_commands in TROGON.md blocks a bash command with matching prefix.
#[tokio::test]
async fn deny_command_from_trogon_md_blocks_bash() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("TROGON.md"),
        "## Permissions\ndeny_commands: rm -rf\n",
    ).unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    push_function_call(&mock, "cid-bash", "bash", r#"{"command":"rm -rf /"}"#);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await.unwrap().session_id.to_string();

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("run")])).await.unwrap();

    assert!(
        second_call_has_denied_output(&mock, "cid-bash"),
        "bash 'rm -rf /' must be blocked by deny_commands: rm -rf"
    );

    let audit = agent.test_session_audit_log(&sid).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].tool, "bash");
    assert_eq!(audit[0].outcome, AuditOutcome::Denied, "audit must record Denied for denied bash command");
}

/// Permission rules injected via set_session_config_option("permissions", ...) deny the tool.
#[tokio::test]
async fn permission_rules_via_config_option_denies_tool() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    // TROGON.md has no permission rules — deny comes from config option only
    std::fs::write(dir.path().join("TROGON.md"), "# Project\nNo rules here.\n").unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    push_function_call(&mock, "cid-cfg", "read_file", r#"{"path":".env"}"#);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await.unwrap().session_id.to_string();

    agent.set_session_config_option(SetSessionConfigOptionRequest::new(
        sid.clone(),
        "permissions",
        "## Permissions\ndeny_paths: .env\n",
    )).await.unwrap();

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")])).await.unwrap();

    assert!(
        second_call_has_denied_output(&mock, "cid-cfg"),
        "deny rule injected via set_session_config_option must block matching file tool"
    );

    let audit = agent.test_session_audit_log(&sid).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].outcome, AuditOutcome::Denied,
        "audit must record Denied when rule comes from config option"
    );
}

/// Multiple tool calls in one prompt: one Allowed (README.md), one Denied (secrets/).
#[tokio::test]
async fn mixed_allowed_and_denied_tools_in_same_prompt() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("TROGON.md"),
        "## Permissions\ndeny_paths: secrets/**\n",
    ).unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "r1".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid-ok".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"README.md"}"#.to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "cid-bad".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"secrets/key.pem"}"#.to_string(),
        },
        XaiEvent::Done,
    ]);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await.unwrap().session_id.to_string();

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read both")])).await.unwrap();

    let audit = agent.test_session_audit_log(&sid).await;
    assert_eq!(audit.len(), 2, "audit must have one entry per tool call");

    let has_allowed = audit.iter().any(|e| e.outcome == AuditOutcome::Allowed);
    let has_denied  = audit.iter().any(|e| e.outcome == AuditOutcome::Denied);
    assert!(has_allowed, "audit must contain Allowed entry for README.md");
    assert!(has_denied,  "audit must contain Denied entry for secrets/key.pem");

    let denied = audit.iter().find(|e| e.outcome == AuditOutcome::Denied).unwrap();
    assert!(
        denied.input_summary.contains("secrets"),
        "denied entry must reference the secrets path; got: {}",
        denied.input_summary
    );

    // The denied tool must produce "permission denied" in the follow-up HTTP call
    let calls = mock.calls.lock().unwrap();
    let second_call = calls.get(1).expect("must make a second HTTP call after tool dispatch");
    let denied_output = second_call.input.iter().any(|item| {
        matches!(item, InputItem::FunctionCallOutput { call_id, output }
            if call_id == "cid-bad" && output.contains("permission denied"))
    });
    assert!(denied_output, "denied tool must produce 'permission denied' in follow-up HTTP call");
}

/// A denied tool must NOT get ToolCall(InProgress), only Pending then ToolCallUpdate(Completed).
#[tokio::test]
async fn denied_tool_has_no_inprogress_notification() {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("TROGON.md"),
        "## Permissions\ndeny_paths: .env\n",
    ).unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    push_function_call(&mock, "cid-denied", "read_file", r#"{"path":".env"}"#);
    mock.push_response(done_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await.unwrap().session_id.to_string();

    agent.prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("read")])).await.unwrap();

    let notifs = agent.test_notifier().notifications.lock().unwrap();

    let has_pending = notifs.iter().any(|n| {
        matches!(&n.update, SessionUpdate::ToolCall(tc)
            if tc.tool_call_id.to_string() == "cid-denied"
                && tc.status == ToolCallStatus::Pending)
    });
    assert!(has_pending, "ToolCall(Pending) must be sent when FunctionCall event is received; got: {notifs:?}");

    let has_inprogress = notifs.iter().any(|n| {
        matches!(&n.update, SessionUpdate::ToolCall(tc)
            if tc.tool_call_id.to_string() == "cid-denied"
                && tc.status == ToolCallStatus::InProgress)
    });
    assert!(!has_inprogress, "ToolCall(InProgress) must NOT be sent for a denied tool; got: {notifs:?}");

    let has_denied_update = notifs.iter().any(|n| {
        if let SessionUpdate::ToolCallUpdate(tcu) = &n.update {
            tcu.tool_call_id.to_string() == "cid-denied"
                && tcu.fields.raw_output.as_ref()
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
}
