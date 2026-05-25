//! Cross-runner integration tests: export/import between codex-runner and other runners.
//!
//! Exercises two directions:
//!  1. codex → openrouter: build history in codex via a real prompt through the mock
//!     codex server, export it, import into openrouter, verify role/text fidelity.
//!  2. xai → codex: seed xai history, export, import into codex, verify that the
//!     imported messages are immediately visible via session/export before any prompt.
//!
//! Requires `CODEX_BIN` to point to the `mock_codex_server` binary compiled from this
//! crate. `BIN_ENV_LOCK` serialises env-var writes across tests within this binary.

use std::sync::Arc;
use std::sync::OnceLock;

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{Agent as _, ContentBlock, ExtRequest, NewSessionRequest, PromptRequest, TextContent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::LocalSet;
use trogon_codex_runner::DefaultCodexAgent;
use trogon_runner_tools::portable_session::{PortableBlock, PortableMessage};

use trogon_openrouter_runner::{MockOpenRouterHttpClient, MockSessionNotifier as OrMockNotifier, OpenRouterAgent};
use trogon_xai_runner::{Message as XaiMessage, MockSessionNotifier as XaiMockNotifier, MockXaiHttpClient, XaiAgent};

// ── shared helpers ────────────────────────────────────────────────────────────

const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock_codex_server");

static BIN_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bin_env_lock() -> &'static Mutex<()> {
    BIN_ENV_LOCK.get_or_init(Mutex::default)
}

/// Minimal fake NATS that handles the INFO/CONNECT/PING handshake only.
/// Sufficient for CodexAgent's fire-and-forget session notifications.
async fn fake_nats() -> async_nats::Client {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            writer
                .write_all(
                    b"INFO {\"server_id\":\"test\",\"version\":\"2.10.0\",\
                      \"max_payload\":1048576,\"proto\":1,\"headers\":true}\r\n",
                )
                .await
                .ok();
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("CONNECT") {
                    writer.write_all(b"+OK\r\n").await.ok();
                } else if line.starts_with("PING") {
                    writer.write_all(b"PONG\r\n").await.ok();
                }
            }
        }
    });
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .unwrap()
}

async fn make_codex_agent() -> DefaultCodexAgent {
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
    DefaultCodexAgent::with_nats(
        fake_nats().await,
        AcpPrefix::new("test").unwrap(),
        "o4-mini",
    )
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Build a session in codex-runner via a real prompt through the mock server,
/// export it as portable messages, import into openrouter-runner, re-export,
/// and verify role/text fidelity across the codex→openrouter boundary.
#[tokio::test(flavor = "current_thread")]
async fn cross_runner_codex_export_into_openrouter_import() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_SEND_N_TEXT_EVENTS", "1") };

    let local = LocalSet::new();
    local
        .run_until(async {
            // ── 1. Build codex session history via a prompt ───────────────────
            let codex_agent = make_codex_agent().await;

            let src = codex_agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let src_id = src.session_id.to_string();

            codex_agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("question"))],
                ))
                .await
                .unwrap();

            // ── 2. Export from codex ──────────────────────────────────────────
            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": src_id }).to_string(),
                )
                .unwrap()
                .into();
            let codex_export_resp = codex_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("codex session/export should succeed");

            let exported_json = codex_export_resp.0.get().to_string();
            let codex_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("codex export should be valid JSON");

            assert_eq!(codex_portable.len(), 2, "codex export must have user + assistant");
            assert_eq!(codex_portable[0].role, "user");
            assert_eq!(codex_portable[0].text, "question");
            assert_eq!(codex_portable[1].role, "assistant");

            // ── 3. Import into openrouter ─────────────────────────────────────
            let or_http = MockOpenRouterHttpClient::new();
            let or_notifier = OrMockNotifier::new();
            let or_agent = OpenRouterAgent::with_deps(or_notifier, "test-model", "", or_http);
            or_agent.test_insert_session("or-codex-s1").await;

            let import_body =
                format!(r#"{{"sessionId":"or-codex-s1","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            or_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("openrouter session/import should succeed");

            // ── 4. Export from openrouter and verify fidelity ─────────────────
            let or_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "or-codex-s1" }).to_string(),
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
                codex_portable.len(),
                or_portable.len(),
                "message count should match after codex→openrouter round-trip"
            );
            for (codex_msg, or_msg) in codex_portable.iter().zip(or_portable.iter()) {
                assert_eq!(
                    codex_msg.role, or_msg.role,
                    "role mismatch in codex→openrouter round-trip"
                );
                assert_eq!(
                    codex_msg.text, or_msg.text,
                    "text mismatch in codex→openrouter round-trip"
                );
            }
        })
        .await;
}

/// Export a session from xai-runner and import it into codex-runner. Verifies
/// that the imported messages are immediately visible via session/export before
/// any prompt — consistent with how acp-runner and openrouter-runner behave.
#[tokio::test(flavor = "current_thread")]
async fn cross_runner_xai_export_into_codex_import() {
    let _guard = bin_env_lock().lock().await;

    let local = LocalSet::new();
    local
        .run_until(async {
            // ── 1. Seed xai session and export ───────────────────────────────
            let xai_http = Arc::new(MockXaiHttpClient::new());
            let xai_notifier = Arc::new(XaiMockNotifier::new());
            let xai_agent =
                XaiAgent::with_deps(xai_notifier, "grok-3", "test-key", xai_http);

            xai_agent
                .test_insert_session_with_history(
                    "xai-for-codex",
                    "/tmp",
                    vec![
                        XaiMessage::user("prior question"),
                        XaiMessage::assistant_text("prior answer"),
                    ],
                )
                .await;

            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "xai-for-codex" }).to_string(),
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
            assert_eq!(xai_portable[0].role, "user");
            assert_eq!(xai_portable[0].text, "prior question");
            assert_eq!(xai_portable[1].role, "assistant");
            assert_eq!(xai_portable[1].text, "prior answer");

            // ── 2. Import into codex ──────────────────────────────────────────
            let codex_agent = make_codex_agent().await;

            let dst = codex_agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let dst_id = dst.session_id.to_string();

            let import_body =
                format!(r#"{{"sessionId":"{}","messages":{exported_json}}}"#, dst_id);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            codex_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("codex session/import should succeed");

            // ── 3. Export from codex before any prompt ────────────────────────
            // Imported messages must be immediately visible — codex stores them
            // in the history field on import, consistent with other runners.
            let codex_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": dst_id }).to_string(),
                )
                .unwrap()
                .into();
            let codex_export_resp = codex_agent
                .ext_method(ExtRequest::new("session/export", codex_export_params))
                .await
                .expect("codex session/export should succeed");

            let codex_portable: Vec<PortableMessage> =
                serde_json::from_str(codex_export_resp.0.get())
                    .expect("codex export should be valid JSON");

            assert_eq!(
                codex_portable.len(),
                xai_portable.len(),
                "codex export before prompt must return the imported messages"
            );
            for (xai_msg, codex_msg) in xai_portable.iter().zip(codex_portable.iter()) {
                assert_eq!(
                    xai_msg.role, codex_msg.role,
                    "role mismatch in xai→codex round-trip"
                );
                assert_eq!(
                    xai_msg.text, codex_msg.text,
                    "text mismatch in xai→codex round-trip"
                );
            }
        })
        .await;
}

// ── acp-style export into codex import ───────────────────────────────────────

/// Import ACP-style export (containing PortableBlock::ToolCall and
/// PortableBlock::ToolResult blocks) into codex-runner.  Codex stores
/// PortableMessages verbatim in its history, so structured blocks must survive
/// a re-export unchanged.
#[tokio::test(flavor = "current_thread")]
async fn cross_runner_acp_style_export_into_codex_import() {
    let _guard = bin_env_lock().lock().await;

    let local = LocalSet::new();
    local
        .run_until(async {
            // Synthetic ACP-style messages: ToolCall (assistant) + ToolResult (user)
            let messages = vec![
                PortableMessage { role: "user".to_string(), text: "use a tool".to_string(), blocks: vec![] },
                PortableMessage {
                    role: "assistant".to_string(),
                    text: String::new(),
                    blocks: vec![PortableBlock::ToolCall {
                        id: "c1".to_string(),
                        name: "str_replace".to_string(),
                        input: serde_json::json!({"path": "f.rs", "old_str": "x", "new_str": "y"}),
                    }],
                },
                PortableMessage {
                    role: "user".to_string(),
                    text: String::new(),
                    blocks: vec![PortableBlock::ToolResult {
                        tool_call_id: "c1".to_string(),
                        content: "OK".to_string(),
                    }],
                },
                PortableMessage { role: "assistant".to_string(), text: "done".to_string(), blocks: vec![] },
            ];
            let exported_json = serde_json::to_string(&messages).unwrap();

            let codex_agent = make_codex_agent().await;
            let dst = codex_agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let dst_id = dst.session_id.to_string();

            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    format!(r#"{{"sessionId":"{}","messages":{}}}"#, dst_id, exported_json),
                )
                .unwrap()
                .into();
            codex_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("codex session/import of ACP-style messages must succeed");

            // Re-export from codex and verify blocks survive verbatim.
            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": dst_id }).to_string(),
                )
                .unwrap()
                .into();
            let codex_export = codex_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("codex session/export must succeed");

            let codex_portable: Vec<PortableMessage> =
                serde_json::from_str(codex_export.0.get())
                    .expect("codex export must be valid JSON");

            assert_eq!(codex_portable.len(), 4, "must have 4 messages after codex import");

            let has_tool_call = codex_portable
                .iter()
                .any(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolCall { .. })));
            assert!(has_tool_call, "ToolCall block must survive codex import/export round-trip");

            let has_tool_result = codex_portable
                .iter()
                .any(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolResult { .. })));
            assert!(has_tool_result, "ToolResult block must survive codex import/export round-trip");
        })
        .await;
}

// ── openrouter-style export into codex import ─────────────────────────────────

/// Import OpenRouter-style export (role:"tool" ToolResult messages) into
/// codex-runner.  Codex stores PortableMessages verbatim in its history:
/// the role and blocks must be preserved unchanged after a re-export.
#[tokio::test(flavor = "current_thread")]
async fn cross_runner_openrouter_style_export_into_codex_import() {
    let _guard = bin_env_lock().lock().await;

    let local = LocalSet::new();
    local
        .run_until(async {
            // OR-style: role:"tool" for ToolResult messages.
            let messages = vec![
                PortableMessage { role: "user".to_string(), text: "use a tool".to_string(), blocks: vec![] },
                PortableMessage {
                    role: "assistant".to_string(),
                    text: String::new(),
                    blocks: vec![PortableBlock::ToolCall {
                        id: "c2".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "test.txt"}),
                    }],
                },
                PortableMessage {
                    role: "tool".to_string(),
                    text: String::new(),
                    blocks: vec![PortableBlock::ToolResult {
                        tool_call_id: "c2".to_string(),
                        content: "file contents".to_string(),
                    }],
                },
                PortableMessage { role: "assistant".to_string(), text: "I read the file".to_string(), blocks: vec![] },
            ];
            let exported_json = serde_json::to_string(&messages).unwrap();

            let codex_agent = make_codex_agent().await;
            let dst = codex_agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let dst_id = dst.session_id.to_string();

            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    format!(r#"{{"sessionId":"{}","messages":{}}}"#, dst_id, exported_json),
                )
                .unwrap()
                .into();
            codex_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("codex session/import of OR-style messages must succeed");

            // Re-export: role:"tool" and ToolResult block must be preserved verbatim.
            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": dst_id }).to_string(),
                )
                .unwrap()
                .into();
            let codex_export = codex_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("codex session/export must succeed");

            let codex_portable: Vec<PortableMessage> =
                serde_json::from_str(codex_export.0.get())
                    .expect("codex export must be valid JSON");

            assert_eq!(codex_portable.len(), 4, "must have 4 messages after codex import");

            let tool_result_msg = codex_portable
                .iter()
                .find(|m| m.blocks.iter().any(|b| matches!(b, PortableBlock::ToolResult { .. })))
                .expect("ToolResult block must survive codex import/export round-trip");
            assert_eq!(
                tool_result_msg.role, "tool",
                "OR role:'tool' must be preserved verbatim by codex (no normalization); got: {}",
                tool_result_msg.role
            );
        })
        .await;
}
