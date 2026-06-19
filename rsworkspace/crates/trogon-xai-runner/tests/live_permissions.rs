//! Live integration tests for granular permissions in `XaiAgent`.
//!
//! These tests hit the **real xAI API** and require a valid API key.
//! They are `#[ignore]` by default and never run in CI.
//!
//! Run with:
//!   cargo test -p trogon-xai-runner --test live_permissions --features test-helpers -- --ignored
//!
//! The `XAI_API_KEY` environment variable must be set to a valid key.
//!
//! Each test creates a real tempdir for TROGON.md, builds an `XaiAgent` with
//! the real HTTP client but a `MockSessionNotifier` (for notification inspection),
//! and drives it through the `Agent` trait without any NATS dependency.

use agent_client_protocol::{
    Agent as _, ContentBlock, NewSessionRequest, PromptRequest, SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use trogon_runner_tools::session_store::AuditOutcome;
use trogon_xai_runner::{MockSessionNotifier, XaiAgent, XaiClient};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a live `XaiAgent` with the real xAI HTTP client and a `MockSessionNotifier`.
///
/// Uses `with_deps` (not `new_in_memory`) so the real `XaiClient` and real
/// `FsTrogonMdLoader` are wired in. The resulting type is
/// `XaiAgent<XaiClient, MockSessionNotifier>` with the default `FsTrogonMdLoader`.
fn make_live_agent(api_key: &str) -> XaiAgent<XaiClient, MockSessionNotifier> {
    XaiAgent::with_deps(MockSessionNotifier::new(), "grok-3", api_key, XaiClient::new())
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Permissions injected via `set_session_config_option("permissions", ...)` deny
/// the matching path tool. The model does NOT see the deny rule in the system
/// prompt (TROGON.md has no Permissions section), so it will genuinely try to
/// call `read_file` — and the runner will block it.
///
/// After the prompt we inspect the audit log and assert a Denied entry exists.
#[ignore = "requires XAI_API_KEY — see file header"]
#[tokio::test]
async fn live_deny_path_via_config_option_model_gets_blocked() {
    let api_key =
        std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set to run live permission tests — see file header");

    let dir = tempfile::TempDir::new().unwrap();

    // Write a real .env file with a known value so the model has something to find.
    std::fs::write(dir.path().join(".env"), "TEST_VALUE=hello\n").unwrap();

    // TROGON.md has no Permissions section — deny comes from config option only.
    // The model will not see the deny rule and will attempt to call read_file.
    std::fs::write(
        dir.path().join("TROGON.md"),
        "# Project\nYou have access to file reading tools.\n",
    )
    .unwrap();

    let agent = make_live_agent(&api_key);

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await
        .unwrap()
        .session_id
        .to_string();

    // Inject the deny rule after session creation via config option.
    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "permissions",
            "## Permissions\ndeny_paths: .env\n",
        ))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::from(
                "Read the file .env and tell me what TEST_VALUE is set to. Use the read_file tool.",
            )],
        ))
        .await
        .unwrap();

    let audit = agent.test_session_audit_log(&sid).await;

    assert!(
        !audit.is_empty(),
        "model did not call read_file tool — try running again (audit log is empty)"
    );

    let denied = audit
        .iter()
        .any(|e| e.tool == "read_file" && e.outcome == AuditOutcome::Denied);
    assert!(
        denied,
        "expected a Denied audit entry for read_file on .env; got: {audit:?}"
    );
}

/// When `bypassPermissions` is set in the `new_session` meta and `.env` is in
/// `deny_paths` (via TROGON.md), the runner must allow the tool call anyway.
///
/// Because the deny rule comes from TROGON.md (which becomes the system prompt),
/// the model MAY refuse on its own. This test therefore only verifies that if
/// the model does attempt the tool, the runner records Allowed in the audit log.
/// We also assert the prompt response is non-empty (model processed something).
#[ignore = "requires XAI_API_KEY — see file header"]
#[tokio::test]
async fn live_bypass_permissions_allows_denied_path() {
    let api_key =
        std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set to run live permission tests — see file header");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join(".env"), "TEST_VALUE=hello\n").unwrap();

    // TROGON.md has deny_paths — this becomes the system prompt so the model
    // will likely see the restriction. bypassPermissions must override the runner
    // check regardless of what the model decides to do.
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let agent = make_live_agent(&api_key);

    let mut meta = serde_json::Map::new();
    meta.insert("bypassPermissions".to_string(), serde_json::json!(true));

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()).meta(meta))
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
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::from(
                "Read the file .env and tell me what TEST_VALUE is set to. Use the read_file tool.",
            )],
        ))
        .await
        .unwrap();

    let audit = agent.test_session_audit_log(&sid).await;

    // If the model called the tool, every entry must be Allowed (none Denied).
    let has_denied = audit.iter().any(|e| e.outcome == AuditOutcome::Denied);
    assert!(
        !has_denied,
        "bypassPermissions must not produce Denied entries; got audit: {audit:?}"
    );

    // Verify the session produced a non-empty history (model responded).
    let history = agent.test_session_history(&sid).await;
    assert!(
        !history.is_empty(),
        "expected non-empty session history after prompt; model may have failed"
    );
}

/// `set_session_mode("bypassPermissions")` called after `new_session` must
/// override deny rules loaded from TROGON.md. Specifically, when the runner
/// checks permissions for `read_file` on `.env`, it must see bypass mode and
/// record Allowed in the audit log.
///
/// Since TROGON.md's deny rule appears in the system prompt, the model may
/// choose not to call the tool. We only assert that if a tool call did land,
/// its audit entry is Allowed (not Denied).
#[ignore = "requires XAI_API_KEY — see file header"]
#[tokio::test]
async fn live_set_session_mode_bypass_overrides_trogon_md_deny() {
    let api_key =
        std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set to run live permission tests — see file header");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join(".env"), "SECRET=abc\n").unwrap();

    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let agent = make_live_agent(&api_key);

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await
        .unwrap()
        .session_id
        .to_string();

    // Switch to bypass mode after session creation.
    agent
        .set_session_mode(SetSessionModeRequest::new(sid.clone(), "bypassPermissions"))
        .await
        .unwrap();

    assert_eq!(
        agent.test_session_mode(&sid).await.as_deref(),
        Some("bypassPermissions"),
        "session mode must update to bypassPermissions after set_session_mode"
    );

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::from("Read the file .env using read_file. Path is .env.")],
        ))
        .await
        .unwrap();

    let audit = agent.test_session_audit_log(&sid).await;

    // Any tool calls that did land must have been Allowed.
    let has_denied = audit.iter().any(|e| e.outcome == AuditOutcome::Denied);
    assert!(
        !has_denied,
        "set_session_mode bypassPermissions must not produce Denied entries; audit: {audit:?}"
    );
}

/// A `deny_commands` rule injected via `set_session_config_option("permissions", ...)`
/// must block a matching bash command. The model will NOT see the deny rule in
/// the system prompt (TROGON.md has no Permissions section), so it will
/// genuinely attempt to call the `bash` tool with the requested command.
#[ignore = "requires XAI_API_KEY — see file header"]
#[tokio::test]
async fn live_deny_command_via_config_option_blocks_bash() {
    let api_key =
        std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set to run live permission tests — see file header");

    let dir = tempfile::TempDir::new().unwrap();

    // TROGON.md with no Permissions section — model sees no restrictions.
    std::fs::write(dir.path().join("TROGON.md"), "# Project\nYou have bash access.\n").unwrap();

    let agent = make_live_agent(&api_key);

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await
        .unwrap()
        .session_id
        .to_string();

    // Inject deny_commands rule — model will not see this in the system prompt.
    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "permissions",
            "## Permissions\ndeny_commands: rm -rf\n",
        ))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::from(
                "Run the bash command: rm -rf /tmp/definitely_not_exists_xyz. Use the bash tool.",
            )],
        ))
        .await
        .unwrap();

    let audit = agent.test_session_audit_log(&sid).await;

    assert!(
        !audit.is_empty(),
        "model did not call bash tool — try running again (audit log is empty)"
    );

    let denied = audit
        .iter()
        .any(|e| e.tool == "bash" && e.outcome == AuditOutcome::Denied);
    assert!(
        denied,
        "expected a Denied audit entry for bash with 'rm -rf'; got: {audit:?}"
    );
}

/// Rules in TROGON.md `## Permissions` section are loaded at `new_session` time
/// and applied to all subsequent tool calls. Because the deny rule appears in the
/// system prompt (TROGON.md content is used as system context), the model may
/// refuse to call the tool on its own.
///
/// Two acceptable outcomes:
///   1. The model tried `read_file` on `.env` and the runner blocked it
///      (audit entry with Denied).
///   2. The model understood from the system prompt that `.env` is off-limits
///      and refused on its own (audit is empty, but response text mentions the
///      restriction).
///
/// The test fails only if neither condition holds — i.e. if the model somehow
/// called the tool AND it was allowed, which would indicate the rules were not
/// loaded from TROGON.md.
///
/// Note: `test_session_audit_log` is a plain pub method on the generic impl
/// (behind the `test-helpers` feature) and does not require any extra feature gate.
#[ignore = "requires XAI_API_KEY — see file header"]
#[tokio::test]
async fn live_trogon_md_deny_path_rule_loaded_at_new_session() {
    let api_key =
        std::env::var("XAI_API_KEY").expect("XAI_API_KEY must be set to run live permission tests — see file header");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join(".env"), "SECRET=from-trogon-md\n").unwrap();

    // Deny rule is in TROGON.md — it will be part of the system prompt.
    std::fs::write(dir.path().join("TROGON.md"), "## Permissions\ndeny_paths: .env\n").unwrap();

    let agent = make_live_agent(&api_key);

    let sid = agent
        .new_session(NewSessionRequest::new(dir.path()))
        .await
        .unwrap()
        .session_id
        .to_string();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::from("Read the file .env using read_file. Path is .env.")],
        ))
        .await
        .unwrap();

    let audit = agent.test_session_audit_log(&sid).await;

    // Check outcome 1: the runner blocked a genuine tool call.
    let runner_blocked = audit
        .iter()
        .any(|e| e.tool == "read_file" && e.outcome == AuditOutcome::Denied);

    if runner_blocked {
        // Happy path — runner enforced the rule.
        return;
    }

    // Check outcome 2: the model refused on its own (audit empty or no read_file call).
    // In this case the model's response text must indicate it cannot read .env.
    let history = agent.test_session_history(&sid).await;
    let response_text: String = history
        .iter()
        .filter(|m| m.role == "assistant")
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    let model_refused = response_text.contains("permission")
        || response_text.contains("cannot")
        || response_text.contains("not allowed")
        || response_text.contains("denied")
        || response_text.contains("restrict")
        || response_text.contains("unable")
        || response_text.contains("prohibited");

    assert!(
        model_refused,
        "expected either: (a) runner blocked read_file with Denied in audit, \
         or (b) model response mentions the restriction — \
         but neither condition was met.\n\
         audit: {audit:?}\n\
         response: {response_text:?}"
    );
}
