//! Live end-to-end test for the three platform features in trogon-acp-runner.
//!
//! Features under test:
//!   1 — Path-scoped RBAC   (PolicyAction, ToolPolicy, RulesPermissionChecker)
//!   2 — Egress policies    (EgressPolicy, MCP server gating in build_session_mcp)
//!   3 — Audit trail        (AuditEntry drain into SessionState.audit_log → NATS KV)
//!
//! Requirements:
//!   • NATS running on nats://localhost:4222  (no auth needed)
//!   • No real Anthropic key — all LLM responses served by a local httpmock server
//!
//! Run:
//!   cargo run -p trogon-e2e --bin platform_live

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    ContentBlock, PromptRequest, RequestPermissionOutcome, RequestPermissionResponse,
    SelectedPermissionOutcome, TextContent,
};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt as _;
use httpmock::prelude::*;
use tokio::sync::{RwLock, mpsc};
use trogon_acp_runner::egress::{EgressAction, EgressPolicy};
use trogon_acp_runner::permission_bridge::handle_permission_request_nats;
use trogon_acp_runner::session_store::{AuditOutcome, PolicyAction, ToolPolicy};
use trogon_acp_runner::{
    GatewayConfig, NatsSessionNotifier, NatsSessionStore, PermissionReq, PermissionTx, SessionState,
    SessionStore, StoredMcpServer, TrogonAgent,
};
use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;
use uuid::Uuid;

// ── Subject helpers ────────────────────────────────────────────────────────────

fn prompt_subject(prefix: &str, session_id: &str) -> String {
    use acp_nats::AcpSessionId;
    acp_nats::nats::session::agent::PromptSubject::new(
        &AcpPrefix::new(prefix).unwrap(),
        &AcpSessionId::new(session_id).unwrap(),
    )
    .to_string()
}

// ── Infrastructure helpers ─────────────────────────────────────────────────────

async fn connect_nats() -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect("nats://localhost:4222")
        .await
        .expect("NATS must be running on localhost:4222 — start it with: nats-server -js");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

fn make_agent(base_url: &str) -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http,
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "mock-token".to_string(),
        anthropic_base_url: Some(base_url.to_string()),
        anthropic_extra_headers: vec![],
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext { proxy_url: "http://127.0.0.1:1".to_string() }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
        streaming_client: None,
    }
}

async fn start_agent(
    nats: async_nats::Client,
    js: &jetstream::Context,
    prefix: &str,
    agent: AgentLoop,
    permission_tx: Option<PermissionTx>,
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
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    );
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let (_, io_task) = AgentSideNatsConnection::new(ta, nats, acp_prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(io_task);
    tokio::time::sleep(Duration::from_millis(50)).await;
    store
}

async fn send_and_wait(
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
        prompt_subject(prefix, session_id),
        inbox,
        Bytes::from(serde_json::to_vec(&req).unwrap()),
    )
    .await
    .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(timeout_secs), resp_sub.next())
        .await
        .unwrap_or_else(|_| panic!("timed out after {timeout_secs}s waiting for response"))
        .expect("subscription closed");
    serde_json::from_slice(&msg.payload).expect("invalid JSON response")
}

// ── Mock Anthropic response bodies ─────────────────────────────────────────────

fn tool_use_body() -> String {
    serde_json::json!({
        "stop_reason": "tool_use",
        "content": [{"type":"tool_use","id":"tu_001","name":"unknown_tool","input":{}}]
    })
    .to_string()
}

fn workspace_tool_use_body() -> String {
    serde_json::json!({
        "stop_reason": "tool_use",
        "content": [{"type":"tool_use","id":"tu_rbac_1","name":"unknown_tool","input":{"path":"/workspace/foo.rs"}}]
    })
    .to_string()
}

fn end_turn_body(text: &str) -> String {
    serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{"type":"text","text":text}],
        "usage": {"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}
    })
    .to_string()
}

// ── Output helpers ─────────────────────────────────────────────────────────────

fn ok(label: &str) {
    println!("  \x1b[32m✓\x1b[0m  {label}");
}

fn ko(label: &str, reason: &str) {
    println!("  \x1b[31m✗\x1b[0m  {label}");
    println!("       {reason}");
}

fn uid() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 1 — Path-scoped RBAC
// ══════════════════════════════════════════════════════════════════════════════

async fn test_rbac_allow_bypasses_channel() -> bool {
    const LABEL: &str = "RBAC — Allow policy auto-approves without touching permission channel";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-rbac-a-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(workspace_tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    NatsSessionStore::open(&js).await.unwrap()
        .save(&session_id, &SessionState {
            tool_policies: vec![ToolPolicy {
                tool: "unknown_tool".to_string(),
                path_pattern: "/workspace/**".to_string(),
                action: PolicyAction::Allow,
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let _store = start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;
            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            if perm_rx.try_recv().is_ok() {
                return Err("permission channel was consulted — Allow policy should have bypassed it".to_string());
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_rbac_deny_bypasses_channel() -> bool {
    const LABEL: &str = "RBAC — Deny policy auto-rejects without touching permission channel";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-rbac-d-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(workspace_tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    NatsSessionStore::open(&js).await.unwrap()
        .save(&session_id, &SessionState {
            tool_policies: vec![ToolPolicy {
                tool: "unknown_tool".to_string(),
                path_pattern: "/workspace/**".to_string(),
                action: PolicyAction::Deny,
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let _store = start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;
            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            if perm_rx.try_recv().is_ok() {
                return Err("permission channel was consulted — Deny policy should have bypassed it".to_string());
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 2 — Egress policies
// ══════════════════════════════════════════════════════════════════════════════

async fn test_egress_deny_skips_mcp() -> bool {
    const LABEL: &str = "Egress — Deny-all policy: MCP server receives 0 requests";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-egress-{id}");
    let session_id = format!("sess-{id}");

    let mcp = MockServer::start();
    let mcp_init = mcp.mock(|when, then| {
        when.method(POST).body_contains("\"initialize\"");
        then.status(200).header("Content-Type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"blocked"}}}"#);
    });

    let anthropic = MockServer::start();
    anthropic.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });

    NatsSessionStore::open(&js).await.unwrap()
        .save(&session_id, &SessionState {
            mcp_servers: vec![StoredMcpServer {
                name: "blocked_srv".to_string(),
                url: mcp.base_url(),
                headers: vec![],
            }],
            egress_policy: Some(EgressPolicy {
                default_action: EgressAction::Deny,
                rules: vec![],
            }),
            ..Default::default()
        })
        .await
        .unwrap();

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let _store = start_agent(nats.clone(), &js, &prefix, make_agent(&anthropic.base_url()), None).await;
            let resp = send_and_wait(&nats, &prefix, &session_id, "hello", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            let hits = mcp_init.hits();
            if hits != 0 {
                return Err(format!("MCP server received {hits} request(s) — expected 0"));
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ══════════════════════════════════════════════════════════════════════════════
// Feature 3 — Audit trail
// ══════════════════════════════════════════════════════════════════════════════

async fn test_audit_allow_policy_records_allowed() -> bool {
    const LABEL: &str = "Audit — Allow policy records AuditOutcome::Allowed in NATS KV";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-audit-allow-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(workspace_tool_use_body());
    });

    let (perm_tx, _perm_rx) = mpsc::channel::<PermissionReq>(8);

    NatsSessionStore::open(&js).await.unwrap()
        .save(&session_id, &SessionState {
            tool_policies: vec![ToolPolicy {
                tool: "unknown_tool".to_string(),
                path_pattern: "/workspace/**".to_string(),
                action: PolicyAction::Allow,
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store = start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;
            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = store.load(&session_id).await.unwrap();
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::Allowed) {
                return Err(format!("no Allowed entry in audit_log; got: {:?}", state.audit_log));
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_audit_deny_policy_records_denied() -> bool {
    const LABEL: &str = "Audit — Deny policy records AuditOutcome::Denied in NATS KV";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-audit-deny-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(workspace_tool_use_body());
    });

    let (perm_tx, _perm_rx) = mpsc::channel::<PermissionReq>(8);

    NatsSessionStore::open(&js).await.unwrap()
        .save(&session_id, &SessionState {
            tool_policies: vec![ToolPolicy {
                tool: "unknown_tool".to_string(),
                path_pattern: "/workspace/**".to_string(),
                action: PolicyAction::Deny,
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store = start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;
            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = store.load(&session_id).await.unwrap();
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::Denied) {
                return Err(format!("no Denied entry in audit_log; got: {:?}", state.audit_log));
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_audit_channel_approval() -> bool {
    const LABEL: &str = "Audit — Channel approval records RequiredApproval + ApprovedByUser in NATS KV";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-audit-appr-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store = start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;

            tokio::spawn(async move {
                while let Some(req) = perm_rx.recv().await {
                    let _ = req.response_tx.send(true);
                }
            });

            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = store.load(&session_id).await.unwrap();
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::RequiredApproval) {
                return Err(format!("no RequiredApproval entry; got: {:?}", state.audit_log));
            }
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::ApprovedByUser) {
                return Err(format!("no ApprovedByUser entry; got: {:?}", state.audit_log));
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_audit_channel_denial() -> bool {
    const LABEL: &str = "Audit — Channel denial records RequiredApproval + DeniedByUser in NATS KV";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-audit-deny-ch-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store = start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;

            tokio::spawn(async move {
                while let Some(req) = perm_rx.recv().await {
                    let _ = req.response_tx.send(false);
                }
            });

            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = store.load(&session_id).await.unwrap();
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::RequiredApproval) {
                return Err(format!("no RequiredApproval entry; got: {:?}", state.audit_log));
            }
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::DeniedByUser) {
                return Err(format!("no DeniedByUser entry; got: {:?}", state.audit_log));
            }
            Ok(())
        })
        .await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ══════════════════════════════════════════════════════════════════════════════
// Permission bridge helpers
// ══════════════════════════════════════════════════════════════════════════════

/// The NATS subject the ACP client listens on for permission requests.
fn perm_subject(prefix: &str, session_id: &str) -> String {
    format!("{prefix}.session.{session_id}.client.session.request_permission")
}

/// Serialized `RequestPermissionResponse` for a given option_id.
fn perm_response(option_id: &str) -> Bytes {
    serde_json::to_vec(&RequestPermissionResponse::new(
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id.to_string())),
    ))
    .unwrap()
    .into()
}

// ── Permission bridge — Feature 1 + 3 exercised through real NATS ─────────────

/// The permission bridge (`handle_permission_request_nats`) publishes the
/// permission request over real NATS request-reply.  A mock ACP subscriber
/// responds with `allow`.  The decision flows back through the bridge to the
/// agent and the outcome is persisted in the audit log.
async fn test_permission_bridge_allow_via_nats() -> bool {
    const LABEL: &str =
        "Permission bridge — allow via real NATS request-reply → audit log has RequiredApproval+ApprovedByUser";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-bridge-a-{id}");
    let session_id = format!("sess-{id}");
    let subject = perm_subject(&prefix, &session_id);

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store =
                start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;

            // Mock ACP client: subscribe before the prompt so the reply is ready.
            let nats_mock = nats.clone();
            let mut perm_sub = nats.subscribe(subject.clone()).await.unwrap();
            tokio::task::spawn_local(async move {
                while let Some(msg) = perm_sub.next().await {
                    if let Some(reply) = msg.reply {
                        nats_mock.publish(reply, perm_response("allow")).await.ok();
                    }
                }
            });

            // Wire the permission bridge exactly as main.rs does.
            let store_bridge = store.clone();
            let nats_bridge = nats.clone();
            let prefix_bridge = AcpPrefix::new(&prefix).unwrap();
            tokio::task::spawn_local(async move {
                while let Some(req) = perm_rx.recv().await {
                    handle_permission_request_nats(
                        req,
                        nats_bridge.clone(),
                        prefix_bridge.clone(),
                        &store_bridge,
                    )
                    .await;
                }
            });

            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 20).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let state = store.load(&session_id).await.unwrap();
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::RequiredApproval) {
                return Err(format!("no RequiredApproval in audit_log; got: {:?}", state.audit_log));
            }
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::ApprovedByUser) {
                return Err(format!("no ApprovedByUser in audit_log; got: {:?}", state.audit_log));
            }
            Ok(())
        })
        .await;

    match result {
        Ok(()) => { ok(LABEL); true }
        Err(e) => { ko(LABEL, &e); false }
    }
}

/// `allow_always` via the NATS bridge persists the tool to `session.allowed_tools`
/// in NATS KV.  On the next prompt the agent loads the updated session and
/// auto-approves the tool without sending another permission request over NATS.
///
/// Note: verifying the `Allowed` audit entry on the second turn requires
/// a clean message history (the multi-turn history contains prior tool_result
/// entries that confuse a naive `body_contains` mock).  That audit check is
/// covered by `test_allowed_tools_in_session_records_allowed_audit` (test 10).
async fn test_permission_bridge_allow_always_persists_and_auto_approves() -> bool {
    const LABEL: &str =
        "Permission bridge — allow_always writes allowed_tools to KV → second turn auto-approves (no NATS request)";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-bridge-aa-{id}");
    let session_id = format!("sess-{id}");
    let subject = perm_subject(&prefix, &session_id);

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store =
                start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;

            // Count how many NATS permission requests arrive; respond allow_always to the first.
            let nats_mock = nats.clone();
            let mut perm_sub = nats.subscribe(subject.clone()).await.unwrap();
            let request_count = Arc::new(AtomicU32::new(0));
            let rc = request_count.clone();
            tokio::task::spawn_local(async move {
                while let Some(msg) = perm_sub.next().await {
                    rc.fetch_add(1, Ordering::SeqCst);
                    if let Some(reply) = msg.reply {
                        nats_mock.publish(reply, perm_response("allow_always")).await.ok();
                    }
                }
            });

            // Wire the bridge.
            let store_bridge = store.clone();
            let nats_bridge = nats.clone();
            let prefix_bridge = AcpPrefix::new(&prefix).unwrap();
            tokio::task::spawn_local(async move {
                while let Some(req) = perm_rx.recv().await {
                    handle_permission_request_nats(
                        req,
                        nats_bridge.clone(),
                        prefix_bridge.clone(),
                        &store_bridge,
                    )
                    .await;
                }
            });

            // ── Turn 1: permission needed, bridge sends allow_always ─────────
            let resp1 = send_and_wait(&nats, &prefix, &session_id, "use a tool first time", 20).await;
            if resp1["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("turn 1 unexpected stopReason: {resp1}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;

            // allowed_tools must now contain the tool in NATS KV.
            let state1 = store.load(&session_id).await.unwrap();
            if !state1.allowed_tools.contains(&"unknown_tool".to_string()) {
                return Err(format!(
                    "expected unknown_tool in allowed_tools after allow_always; got: {:?}",
                    state1.allowed_tools
                ));
            }

            // ── Turn 2: tool auto-approved from allowed_tools, no NATS request ─
            let hits_before = request_count.load(Ordering::SeqCst);
            let resp2 = send_and_wait(&nats, &prefix, &session_id, "use a tool second time", 20).await;
            if resp2["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("turn 2 unexpected stopReason: {resp2}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;

            let hits_after = request_count.load(Ordering::SeqCst);
            if hits_after != hits_before {
                return Err(format!(
                    "turn 2 sent {} NATS permission request(s) — should be 0 (auto-approved from allowed_tools)",
                    hits_after - hits_before
                ));
            }

            Ok(())
        })
        .await;

    match result {
        Ok(()) => { ok(LABEL); true }
        Err(e) => { ko(LABEL, &e); false }
    }
}

/// When a session already has `allowed_tools = ["unknown_tool"]` in NATS KV,
/// the agent auto-approves the tool without consulting the permission channel
/// and records `AuditOutcome::Allowed` in the audit log.
///
/// This is the audit-recording companion to test 9.  It uses a fresh session
/// (no multi-turn history) so the mock's `body_contains("tool_result")`
/// discriminator works reliably.
async fn test_allowed_tools_in_session_records_allowed_audit() -> bool {
    const LABEL: &str =
        "Auto-approve — allowed_tools in session state auto-approves and records Allowed in audit log";
    let (nats, js) = connect_nats().await;
    let id = uid();
    let prefix = format!("live-aa-audit-{id}");
    let session_id = format!("sess-{id}");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200).header("Content-Type", "application/json").body(end_turn_body("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200).header("Content-Type", "application/json").body(tool_use_body());
    });

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(8);

    // Seed the session with allowed_tools already set — simulating a prior allow_always.
    NatsSessionStore::open(&js)
        .await
        .unwrap()
        .save(
            &session_id,
            &SessionState {
                allowed_tools: vec!["unknown_tool".to_string()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let result: Result<(), String> = tokio::task::LocalSet::new()
        .run_until(async {
            let store =
                start_agent(nats.clone(), &js, &prefix, make_agent(&server.base_url()), Some(perm_tx)).await;

            // Permission channel must never be consulted (auto-approved from allowed_tools).
            tokio::spawn(async move {
                while let Some(req) = perm_rx.recv().await {
                    let _ = req.response_tx.send(false);
                }
            });

            let resp = send_and_wait(&nats, &prefix, &session_id, "use a tool", 15).await;
            if resp["stopReason"].as_str() != Some("end_turn") {
                return Err(format!("unexpected stopReason: {resp}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;

            let state = store.load(&session_id).await.unwrap();
            if !state.audit_log.iter().any(|e| e.outcome == AuditOutcome::Allowed) {
                return Err(format!(
                    "expected Allowed entry in audit_log; got: {:?}",
                    state.audit_log
                ));
            }
            Ok(())
        })
        .await;

    match result {
        Ok(()) => { ok(LABEL); true }
        Err(e) => { ko(LABEL, &e); false }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Entry point
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    println!();
    println!("══════════════════════════════════════════════════════════");
    println!(" trogon-acp-runner — platform features live test");
    println!("  NATS: nats://localhost:4222");
    println!("  Anthropic: mock (httpmock)  — no real credentials");
    println!("══════════════════════════════════════════════════════════");
    println!();
    println!("Feature 1 — Path-scoped RBAC");

    let r1 = test_rbac_allow_bypasses_channel().await;
    let r2 = test_rbac_deny_bypasses_channel().await;

    println!();
    println!("Feature 2 — Egress policies");

    let r3 = test_egress_deny_skips_mcp().await;

    println!();
    println!("Feature 3 — Audit trail");

    let r4 = test_audit_allow_policy_records_allowed().await;
    let r5 = test_audit_deny_policy_records_denied().await;
    let r6 = test_audit_channel_approval().await;
    let r7 = test_audit_channel_denial().await;

    println!();
    println!("Permission bridge (NATS request-reply, not bypassed)");

    let r8 = test_permission_bridge_allow_via_nats().await;
    let r9 = test_permission_bridge_allow_always_persists_and_auto_approves().await;
    let r10 = test_allowed_tools_in_session_records_allowed_audit().await;

    let results = [r1, r2, r3, r4, r5, r6, r7, r8, r9, r10];
    let passed = results.iter().filter(|&&r| r).count();
    let total = results.len();

    println!();
    println!("══════════════════════════════════════════════════════════");
    if passed == total {
        println!(" \x1b[32mAll {total} tests passed\x1b[0m");
    } else {
        println!(" \x1b[31m{passed}/{total} tests passed\x1b[0m");
    }
    println!("══════════════════════════════════════════════════════════");
    println!();

    if passed < total {
        std::process::exit(1);
    }
}
