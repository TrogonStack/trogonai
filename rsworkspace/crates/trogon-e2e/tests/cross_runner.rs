//! Cross-runner integration tests: export/import between xai, openrouter, and acp runners.
//!
//! Verifies that the portable session format is compatible between all runner pairs.
//! xai↔openrouter tests use only mock HTTP clients (no network, no Docker).
//! acp-runner tests require Docker (testcontainers spins up a NATS JetStream server).

use std::sync::Arc;

use agent_client_protocol::{Agent as _, ExtRequest};
use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, SessionState, SessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
};
use trogon_agent_core::agent_loop::{ContentBlock as AgentContentBlock, Message as AgentMessage};
use trogon_runner_tools::portable_session::PortableMessage;

use trogon_xai_runner::{
    Message as XaiMessage, MockSessionNotifier as XaiMockNotifier, MockXaiHttpClient, XaiAgent,
};

use trogon_openrouter_runner::{
    AssembledToolCall, Message as OrMessage, MockOpenRouterHttpClient,
    MockSessionNotifier as OrMockNotifier, OpenRouterAgent,
};
use trogon_runner_tools::portable_session::PortableBlock;

fn local() -> tokio::task::LocalSet {
    tokio::task::LocalSet::new()
}

// ── acp helpers ───────────────────────────────────────────────────────────────

async fn start_nats_js() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn make_js(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

type AcpTestAgent = TrogonAgent<NatsSessionStore, MockAgentRunner, MockSessionNotifier>;

fn make_acp_agent(store: NatsSessionStore) -> AcpTestAgent {
    TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        MockAgentRunner::new("claude-test"),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
}

#[tokio::test]
async fn cross_runner_xai_export_into_openrouter_import() {
    // ── 1. Build xai agent and seed a session with two messages ─────────────────
    let xai_http = Arc::new(MockXaiHttpClient::new());
    let xai_notifier = Arc::new(XaiMockNotifier::new());
    let xai_agent = XaiAgent::with_deps(xai_notifier, "grok-3", "test-key", xai_http);

    xai_agent
        .test_insert_session_with_history(
            "xai-s1",
            "/tmp",
            vec![
                XaiMessage::user("question"),
                XaiMessage::assistant_text("answer"),
            ],
        )
        .await;

    // ── 2. Export from xai-runner ────────────────────────────────────────────────
    let export_params = serde_json::value::RawValue::from_string(
        serde_json::json!({ "sessionId": "xai-s1" }).to_string(),
    )
    .unwrap();
    let xai_export_resp = xai_agent
        .ext_method(ExtRequest::new("session/export", Arc::from(export_params)))
        .await
        .expect("xai session/export should succeed");

    let exported_json = xai_export_resp.0.get().to_string();

    // Sanity-check the xai export
    let xai_portable: Vec<PortableMessage> =
        serde_json::from_str(&exported_json).expect("xai export should be valid JSON");
    assert_eq!(xai_portable.len(), 2);
    assert_eq!(xai_portable[0].role, "user");
    assert_eq!(xai_portable[0].text, "question");
    assert_eq!(xai_portable[1].role, "assistant");
    assert_eq!(xai_portable[1].text, "answer");

    // ── 3. Build openrouter agent (inside LocalSet because it is !Send) ──────────
    local()
        .run_until(async move {
            let or_http = MockOpenRouterHttpClient::new();
            let or_notifier = OrMockNotifier::new();
            let or_agent =
                OpenRouterAgent::with_deps(or_notifier, "test-model", "", or_http);

            // Create an empty openrouter session to import into
            or_agent.test_insert_session("or-s1").await;

            // ── 4. Import the xai export into openrouter ─────────────────────────
            let import_body = format!(
                r#"{{"sessionId":"or-s1","messages":{exported_json}}}"#
            );
            let import_params =
                serde_json::value::RawValue::from_string(import_body).unwrap();
            or_agent
                .ext_method(ExtRequest::new("session/import", Arc::from(import_params)))
                .await
                .expect("openrouter session/import should succeed");

            // ── 5. Export from openrouter ────────────────────────────────────────
            let or_export_params = serde_json::value::RawValue::from_string(
                serde_json::json!({ "sessionId": "or-s1" }).to_string(),
            )
            .unwrap();
            let or_export_resp = or_agent
                .ext_method(ExtRequest::new(
                    "session/export",
                    Arc::from(or_export_params),
                ))
                .await
                .expect("openrouter session/export should succeed");

            let or_portable: Vec<PortableMessage> =
                serde_json::from_str(or_export_resp.0.get())
                    .expect("openrouter export should be valid JSON");

            // ── 6. Assert round-trip fidelity ────────────────────────────────────
            assert_eq!(
                xai_portable.len(),
                or_portable.len(),
                "message count should match after cross-runner round-trip"
            );
            for (xai_msg, or_msg) in xai_portable.iter().zip(or_portable.iter()) {
                assert_eq!(
                    xai_msg.role, or_msg.role,
                    "role mismatch in cross-runner round-trip"
                );
                assert_eq!(
                    xai_msg.text, or_msg.text,
                    "text mismatch in cross-runner round-trip"
                );
            }
        })
        .await;
}

/// Export a session from acp-runner (backed by real NATS KV) and import it
/// into openrouter-runner. Verifies role/text fidelity across the acp→openrouter
/// boundary using a real JetStream store on the source side.
#[tokio::test]
async fn cross_runner_acp_export_into_openrouter_import() {
    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            // ── 1. Seed an acp session in NATS KV ────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let state = SessionState {
                messages: vec![
                    AgentMessage::user_text("question"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("acp-s1", &state).await.unwrap();
            let acp_agent = make_acp_agent(store);

            // ── 2. Export from acp ────────────────────────────────────────────────
            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export_resp = acp_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("acp session/export should succeed");

            let exported_json = acp_export_resp.0.get().to_string();
            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("acp export should be valid JSON");
            assert_eq!(acp_portable.len(), 2);
            assert_eq!(acp_portable[0].role, "user");
            assert_eq!(acp_portable[0].text, "question");
            assert_eq!(acp_portable[1].role, "assistant");
            assert_eq!(acp_portable[1].text, "answer");

            // ── 3. Import into openrouter ─────────────────────────────────────────
            let or_http = MockOpenRouterHttpClient::new();
            let or_notifier = OrMockNotifier::new();
            let or_agent = OpenRouterAgent::with_deps(or_notifier, "test-model", "", or_http);
            or_agent.test_insert_session("or-s1").await;

            let import_body = format!(r#"{{"sessionId":"or-s1","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            or_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("openrouter session/import should succeed");

            // ── 4. Export from openrouter and verify fidelity ─────────────────────
            let or_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "or-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let or_export_resp = or_agent
                .ext_method(ExtRequest::new("session/export", or_export_params))
                .await
                .expect("openrouter session/export should succeed");

            let or_portable: Vec<PortableMessage> =
                serde_json::from_str(or_export_resp.0.get())
                    .expect("openrouter export should be valid JSON");

            assert_eq!(
                acp_portable.len(),
                or_portable.len(),
                "message count should match after acp→openrouter round-trip"
            );
            for (acp_msg, or_msg) in acp_portable.iter().zip(or_portable.iter()) {
                assert_eq!(acp_msg.role, or_msg.role, "role mismatch in acp→openrouter round-trip");
                assert_eq!(acp_msg.text, or_msg.text, "text mismatch in acp→openrouter round-trip");
            }
        })
        .await;
}

/// Export a session from xai-runner and import it into acp-runner (backed by
/// real NATS KV). Verifies role/text fidelity across the xai→acp boundary
/// using a real JetStream store on the destination side.
#[tokio::test]
async fn cross_runner_xai_export_into_acp_import() {
    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            // ── 1. Seed a xai session and export it ───────────────────────────────
            let xai_http = Arc::new(MockXaiHttpClient::new());
            let xai_notifier = Arc::new(XaiMockNotifier::new());
            let xai_agent = XaiAgent::with_deps(xai_notifier, "grok-3", "test-key", xai_http);
            xai_agent
                .test_insert_session_with_history(
                    "xai-s2",
                    "/tmp",
                    vec![
                        XaiMessage::user("hello"),
                        XaiMessage::assistant_text("world"),
                    ],
                )
                .await;

            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "xai-s2" }).to_string(),
                )
                .unwrap()
                .into();
            let xai_export_resp = xai_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("xai session/export should succeed");

            let exported_json = xai_export_resp.0.get().to_string();
            let xai_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("xai export should be valid JSON");
            assert_eq!(xai_portable.len(), 2);

            // ── 2. Import into acp (real NATS KV) ────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let acp_agent = make_acp_agent(store);

            let import_body = format!(r#"{{"sessionId":"acp-s2","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            acp_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("acp session/import should succeed");

            // ── 3. Export from acp and verify fidelity ────────────────────────────
            let acp_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-s2" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export_resp = acp_agent
                .ext_method(ExtRequest::new("session/export", acp_export_params))
                .await
                .expect("acp session/export should succeed");

            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(acp_export_resp.0.get())
                    .expect("acp export should be valid JSON");

            assert_eq!(
                xai_portable.len(),
                acp_portable.len(),
                "message count should match after xai→acp round-trip"
            );
            for (xai_msg, acp_msg) in xai_portable.iter().zip(acp_portable.iter()) {
                assert_eq!(xai_msg.role, acp_msg.role, "role mismatch in xai→acp round-trip");
                assert_eq!(xai_msg.text, acp_msg.text, "text mismatch in xai→acp round-trip");
            }
        })
        .await;
}

/// Export a session from openrouter-runner (which has role:"tool" messages for tool
/// results) and import it into acp-runner.  Verifies Fix 3: acp-runner normalizes
/// role:"tool" → role:"user" during import so the session is valid for Anthropic.
#[tokio::test]
async fn cross_runner_openrouter_export_into_acp_import_normalizes_tool_role() {
    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            // ── 1. Build an openrouter session with a tool call in history ────────
            let or_http = MockOpenRouterHttpClient::new();
            let or_agent = OpenRouterAgent::with_deps(OrMockNotifier::new(), "m", "", or_http);
            or_agent
                .test_insert_session_with_history(
                    "or-tool-s1",
                    vec![
                        OrMessage::user("use a tool"),
                        OrMessage::assistant_tool_calls(&[AssembledToolCall {
                            id: "c1".to_string(),
                            name: "read_file".to_string(),
                            arguments: r#"{"path":"test.txt"}"#.to_string(),
                        }]),
                        OrMessage::tool_result("c1".to_string(), "file contents"),
                        OrMessage::assistant("I read the file."),
                    ],
                )
                .await;

            // ── 2. Export from openrouter ─────────────────────────────────────────
            let or_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "or-tool-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let or_export = or_agent
                .ext_method(ExtRequest::new("session/export", or_export_params))
                .await
                .expect("openrouter session/export should succeed");

            let exported_json = or_export.0.get().to_string();
            let or_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("openrouter export should be valid JSON");

            // Sanity-check: openrouter exports tool results with role:"tool"
            let or_tool_msg = or_portable
                .iter()
                .find(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolResult { .. })))
                .expect("OR export must contain a ToolResult block");
            assert_eq!(
                or_tool_msg.role, "tool",
                "OR must export tool results with role:'tool' (OpenAI convention)"
            );

            // ── 3. Import into acp-runner ─────────────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let acp_agent = make_acp_agent(store);

            let import_body =
                format!(r#"{{"sessionId":"acp-tool-s1","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            acp_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("acp session/import should succeed");

            // ── 4. Export from acp and verify role normalization (Fix 3) ──────────
            let acp_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-tool-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export = acp_agent
                .ext_method(ExtRequest::new("session/export", acp_export_params))
                .await
                .expect("acp session/export should succeed");

            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(acp_export.0.get())
                    .expect("acp export should be valid JSON");

            let acp_tool_msg = acp_portable
                .iter()
                .find(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolResult { .. })))
                .expect("ACP export must contain a ToolResult block");
            assert_eq!(
                acp_tool_msg.role, "user",
                "Fix 3: OR role:'tool' must be normalized to role:'user' after ACP import; got: '{}'",
                acp_tool_msg.role
            );
        })
        .await;
}

/// Export a session from acp-runner (which has PortableBlock::ToolCall for tool_use
/// blocks) and import it into xai-runner.  Verifies that xai-runner converts
/// ToolCall blocks to "[called: {name}]" text (xai is text-only, stateful server-side).
#[tokio::test]
async fn cross_runner_acp_export_into_xai_import_converts_tool_calls_to_text() {
    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            // ── 1. Build an acp session with ToolUse + ToolResult blocks ──────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let state = SessionState {
                messages: vec![
                    AgentMessage::user_text("use a tool"),
                    AgentMessage::assistant(vec![AgentContentBlock::ToolUse {
                        id: "c1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "test.txt"}),
                        parent_tool_use_id: None,
                    }]),
                    // ToolResult must be in a user message (Anthropic convention)
                    AgentMessage {
                        role: "user".to_string(),
                        content: vec![AgentContentBlock::ToolResult {
                            tool_use_id: "c1".to_string(),
                            content: "file contents".to_string(),
                        }],
                    },
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "I read the file.".to_string(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("acp-xai-s1", &state).await.unwrap();
            let acp_agent = make_acp_agent(store);

            // ── 2. Export from acp ────────────────────────────────────────────────
            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-xai-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export = acp_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("acp session/export should succeed");

            let exported_json = acp_export.0.get().to_string();
            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("acp export should be valid JSON");

            // Sanity-check: acp exports ToolUse as PortableBlock::ToolCall
            let has_tool_call = acp_portable
                .iter()
                .any(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolCall { .. })));
            assert!(has_tool_call, "ACP export must contain PortableBlock::ToolCall");

            // ── 3. Import into xai-runner ─────────────────────────────────────────
            let xai_http = Arc::new(MockXaiHttpClient::new());
            let xai_notifier = Arc::new(XaiMockNotifier::new());
            let xai_agent =
                XaiAgent::with_deps(xai_notifier, "grok-3", "test-key", xai_http);
            xai_agent
                .test_insert_session_with_history("xai-acp-s1", "/tmp", vec![])
                .await;

            let import_body =
                format!(r#"{{"sessionId":"xai-acp-s1","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            xai_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("xai session/import should succeed");

            // ── 4. Export from xai and verify ToolCall → "[called: read_file]" ────
            let xai_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "xai-acp-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let xai_export = xai_agent
                .ext_method(ExtRequest::new("session/export", xai_export_params))
                .await
                .expect("xai session/export should succeed");

            let xai_portable: Vec<PortableMessage> =
                serde_json::from_str(xai_export.0.get())
                    .expect("xai export should be valid JSON");

            let tool_text_msg = xai_portable
                .iter()
                .find(|m| m.text.contains("[called: read_file]"))
                .expect("XAI export must contain '[called: read_file]' text for the ToolCall block");
            assert_eq!(
                tool_text_msg.role, "assistant",
                "tool call converted message must have role 'assistant'"
            );
        })
        .await;
}

/// Export a session from openrouter-runner after a real tool-call dispatch cycle
/// and import it into acp-runner.  Unlike the pre-seeded tests above, this test
/// runs an actual `agent.prompt()` on OR so the history is built by the agent
/// loop (not manually inserted).  Verifies that the PortableBlock::ToolCall /
/// PortableBlock::ToolResult blocks survive the full OR-prompt → export → ACP-import chain.
#[tokio::test]
async fn cross_runner_or_real_tool_cycle_then_import_into_acp_preserves_tool_blocks() {
    use trogon_openrouter_runner::MockSessionNotifier as OrMockNotifier2;

    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            use std::sync::Arc;
            use agent_client_protocol::{Agent as _, ContentBlock, NewSessionRequest, PromptRequest};

            // ── 1. OR prompt cycle: tool call → dispatch → done ───────────────────
            let http = Arc::new(MockOpenRouterHttpClient::new());
            // First response: read_file tool call
            http.push_response(vec![trogon_openrouter_runner::OpenRouterEvent::ToolCallsReady {
                calls: vec![trogon_openrouter_runner::AssembledToolCall {
                    id: "call_rf_cr".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"cross_runner_test.txt"}"#.to_string(),
                }],
            }]);
            // Second response: done
            http.push_response(vec![trogon_openrouter_runner::OpenRouterEvent::TextDelta {
                text: "file read".to_string(),
            }]);

            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("cross_runner_test.txt"), "real content").unwrap();

            let or_agent = OpenRouterAgent::with_deps(
                OrMockNotifier2::new(),
                "test-model",
                "test-key",
                Arc::clone(&http),
            );

            let new_resp = or_agent
                .new_session(NewSessionRequest::new(dir.path().to_path_buf()))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            or_agent
                .prompt(PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("read the file")],
                ))
                .await
                .unwrap();

            // ── 2. Export from OR (history now has real tool calls) ───────────────
            let or_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": sid }).to_string(),
                )
                .unwrap()
                .into();
            let or_export = or_agent
                .ext_method(ExtRequest::new("session/export", or_export_params))
                .await
                .expect("OR session/export should succeed");

            let exported_json = or_export.0.get().to_string();
            let or_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("OR export should be valid JSON");

            // Must have a ToolCall block (from the assistant's tool_calls message)
            let has_tool_call = or_portable
                .iter()
                .any(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolCall { .. })));
            assert!(
                has_tool_call,
                "OR export after real prompt must contain PortableBlock::ToolCall; got: {or_portable:?}"
            );

            // Must have a ToolResult block (from the role:"tool" result message)
            let has_tool_result = or_portable
                .iter()
                .any(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolResult { .. })));
            assert!(
                has_tool_result,
                "OR export after real prompt must contain PortableBlock::ToolResult; got: {or_portable:?}"
            );

            // ── 3. Import into acp-runner ─────────────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let acp_agent = make_acp_agent(store);

            let import_body =
                format!(r#"{{"sessionId":"acp-real-tool-s1","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            acp_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("acp session/import should succeed");

            // ── 4. Re-export from ACP and verify blocks survived ──────────────────
            let acp_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-real-tool-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export = acp_agent
                .ext_method(ExtRequest::new("session/export", acp_export_params))
                .await
                .expect("acp session/export should succeed");

            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(acp_export.0.get())
                    .expect("acp export should be valid JSON");

            // ACP export must contain ToolCall block (re-exported from imported blocks)
            let acp_has_tool_call = acp_portable
                .iter()
                .any(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolCall { .. })));
            assert!(
                acp_has_tool_call,
                "ACP export after import must preserve PortableBlock::ToolCall; got: {acp_portable:?}"
            );

            // Fix 3: the ToolResult message must have role:"user" (not "tool")
            let acp_tool_result_msg = acp_portable
                .iter()
                .find(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolResult { .. })))
                .expect("ACP export must contain a ToolResult block");
            assert_eq!(
                acp_tool_result_msg.role, "user",
                "Fix 3: role:'tool' from OR must be normalized to role:'user' after ACP import"
            );
        })
        .await;
}
