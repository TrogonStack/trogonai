#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration test for `CrossRunnerSwitcher` against two real `TrogonAgent` instances.
//!
//! Spins up a NATS JetStream server (testcontainers), starts two TrogonAgent
//! instances on different ACP prefixes ("acp.src" and "acp.dst"), seeds the
//! source session with known messages, then drives `CrossRunnerSwitcher::switch_model`
//! end-to-end through real NATS request-reply and verifies the migrated session.
//!
//! Requires Docker.

use std::sync::Arc;
use std::time::Duration;

use acp_nats::{AcpPrefix, Config};
use acp_nats_agent::AgentSideNatsConnection;
use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, SessionState, SessionStore, TrogonAgent, agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
};
use trogon_agent_core::agent_loop::{ContentBlock as AgentContentBlock, Message as AgentMessage};
use trogon_cli::CrossRunnerSwitcher;
use trogon_cli::session_kernel::SessionKernelStack;
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_openrouter_runner::{MockOpenRouterHttpClient, NatsSessionNotifier as OrNatsNotifier, OpenRouterAgent};
use trogon_registry::{AgentCapability, MockRegistryStore, Registry};
use trogon_xai_runner::{MockXaiHttpClient, NatsSessionNotifier as XaiNatsNotifier, XaiAgent};
use trogonai_session_contracts::SessionId;
use trogonai_session_kernel::{SessionKernelFeatureFlags, SessionKernelOperationalPolicy};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats_js() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

/// NATS 2.11+ JetStream server. The Session Kernel provisions a lease KV bucket with
/// per-key TTL (limit markers), which requires NATS 2.11; the default image is older.
async fn start_nats_js_v211() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .with_tag("2.11-alpine")
        .start()
        .await
        .expect("Failed to start NATS 2.11 container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn make_nats(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

type TestAgent = TrogonAgent<NatsSessionStore, MockAgentRunner, MockSessionNotifier>;

fn make_agent(store: NatsSessionStore, prefix: &str) -> TestAgent {
    TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        MockAgentRunner::new("test-model"),
        prefix,
        "test-model",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
}

/// Attach a `TrogonAgent` to NATS via `AgentSideNatsConnection` and spawn its
/// I/O task as a local task. Must be called from within a `LocalSet`.
fn attach_agent(agent: TestAgent, nats: async_nats::Client, prefix: &str) {
    let acp_prefix = AcpPrefix::new(prefix).expect("prefix must be valid");
    let (_, io_task) = AgentSideNatsConnection::new(agent, nats, acp_prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(async move {
        io_task.await.ok();
    });
}

fn make_config(port: u16) -> Config {
    Config::new(
        AcpPrefix::new("acp.src").unwrap(),
        NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: NatsAuth::None,
        },
    )
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `CrossRunnerSwitcher::switch_model` migrates a session from one real
/// TrogonAgent to another over actual NATS JetStream.
///
/// The test seeds messages directly into the source KV store, drives the
/// switcher end-to-end (export→new_session→import over NATS), then reads the
/// destination session from the shared KV bucket to verify the messages arrived.
#[tokio::test]
async fn switch_model_migrates_history_between_two_acp_runners() {
    let (_c, port) = start_nats_js().await;
    let (nats, js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Open shared KV store and seed source session ───────────────
            let store = NatsSessionStore::open(&js).await.unwrap();

            let src_state = SessionState {
                messages: vec![
                    AgentMessage::user_text("original question"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "original answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("switcher-src-1", &src_state).await.unwrap();

            // ── 2. Start two TrogonAgent instances on different prefixes ───────
            attach_agent(make_agent(store.clone(), "acp.src"), nats.clone(), "acp.src");
            attach_agent(make_agent(store.clone(), "acp.dst"), nats.clone(), "acp.dst");

            // Allow subscriptions to settle before sending requests.
            tokio::time::sleep(Duration::from_millis(60)).await;

            // ── 3. Build CrossRunnerSwitcher: "dst-model" → "acp.dst" ─────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut dst_cap = AgentCapability::new("dst-runner", ["chat"], "agents.dst.>");
            dst_cap.metadata = serde_json::json!({ "models": ["dst-model"], "acp_prefix": "acp.dst" });
            registry.register(&dst_cap).await.unwrap();

            let mut switcher = CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry);

            // ── 4. Migrate the session ────────────────────────────────────────
            let trogon_cli::cross_runner::SwitchSurface { new_prefix, new_session_id, .. } = switcher
                .switch_model("acp.src", "switcher-src-1", "sonnet", "dst-model", "/tmp")
                .await
                .expect("switch_model should succeed");

            assert_eq!(new_prefix, "acp.dst");
            assert!(!new_session_id.is_empty());

            // ── 5. Verify migrated messages in dst via shared KV bucket ───────
            let dst_state = store.load(&new_session_id).await.unwrap();

            assert_eq!(dst_state.messages.len(), 2, "migrated session must have 2 messages");
            assert_eq!(
                dst_state.messages[0].role, "user",
                "first migrated message must be user"
            );
            assert_eq!(
                dst_state.messages[1].role, "assistant",
                "second migrated message must be assistant"
            );
            assert!(
                matches!(
                    &dst_state.messages[0].content[0],
                    AgentContentBlock::Text { text } if text == "original question"
                ),
                "user message text must match after migration"
            );
            assert!(
                matches!(
                    &dst_state.messages[1].content[0],
                    AgentContentBlock::Text { text } if text == "original answer"
                ),
                "assistant message text must match after migration"
            );
        })
        .await;
}

/// `CrossRunnerSwitcher::switch_model` migrates a session to a runner registered
/// with "codex" agent type and `code_edit` capability.
///
/// Verifies that the registry's `code_edit` capability tag and `acp_prefix`
/// routing work end-to-end: the switcher resolves the model "o4-mini" to the
/// codex-prefixed runner and migrates the session history correctly.
#[tokio::test]
async fn switch_model_migrates_session_to_codex_runner_prefix() {
    let (_c, port) = start_nats_js().await;
    let (nats, js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Seed source session ────────────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();

            let src_state = SessionState {
                messages: vec![
                    AgentMessage::user_text("ask codex"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "acp answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("codex-src-1", &src_state).await.unwrap();

            // ── 2. Start source agent and a mock "codex" agent ───────────────
            attach_agent(make_agent(store.clone(), "acp.src"), nats.clone(), "acp.src");
            attach_agent(make_agent(store.clone(), "acp.codex"), nats.clone(), "acp.codex");
            tokio::time::sleep(Duration::from_millis(60)).await;

            // ── 3. Register "acp.codex" as a codex runner ────────────────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut codex_cap = AgentCapability::new("codex", ["chat", "code_edit"], "acp.codex.agent.>");
            codex_cap.metadata = serde_json::json!({
                "models": ["o4-mini", "o3"],
                "acp_prefix": "acp.codex"
            });
            registry.register(&codex_cap).await.unwrap();

            // ── 4. Migrate via CrossRunnerSwitcher ────────────────────────────
            let mut switcher =
                CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry);
            let trogon_cli::cross_runner::SwitchSurface { new_prefix, new_session_id, .. } = switcher
                .switch_model("acp.src", "codex-src-1", "sonnet", "o4-mini", "/tmp")
                .await
                .expect("switch_model to codex runner must succeed");

            assert_eq!(new_prefix, "acp.codex");
            assert!(!new_session_id.is_empty());

            // ── 5. Verify migrated messages ───────────────────────────────────
            let dst_state = store.load(&new_session_id).await.unwrap();
            assert_eq!(
                dst_state.messages.len(),
                2,
                "migrated codex session must have 2 messages"
            );
            assert_eq!(dst_state.messages[0].role, "user");
            assert_eq!(dst_state.messages[1].role, "assistant");
            assert!(
                matches!(
                    &dst_state.messages[0].content[0],
                    AgentContentBlock::Text { text } if text == "ask codex"
                ),
                "user message must survive migration to codex runner"
            );
        })
        .await;
}

/// `CrossRunnerSwitcher::switch_model` returns a descriptive error when the
/// requested model is not registered in the registry (`find_by_model` → `None`).
///
/// Exercises the first failure branch in `switch_model` without attaching any
/// runner agents — only NATS connectivity and an empty registry are needed.
#[tokio::test]
async fn switch_model_returns_error_when_model_not_in_registry() {
    let (_c, port) = start_nats_js().await;
    let (nats, _js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let registry = Registry::new(MockRegistryStore::new());
            // Registry is empty — no runners registered.
            let mut switcher = CrossRunnerSwitcher::new(nats, make_config(port), registry);

            let result = switcher
                .switch_model("acp.current", "session-xyz", "sonnet", "unknown-model-xyz", "/workspace")
                .await;

            assert_eq!(
                result,
                Err("no runner found for model: unknown-model-xyz".to_string()),
                "switch_model must return a descriptive error when model is not in registry"
            );
        })
        .await;
}

/// `CrossRunnerSwitcher::switch_model` migrates a session from a live XaiAgent to a live
/// OpenRouterAgent over real NATS — exercises the full cross-runner export→new_session→import
/// chain between two different runner implementations.
#[tokio::test]
async fn switch_model_migrates_history_from_xai_to_openrouter() {
    let (_c, port) = start_nats_js().await;
    let (nats, _js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Attach XaiAgent to "acp.xai" ──────────────────────────────
            let xai_prefix = AcpPrefix::new("acp.xai").unwrap();
            let xai_notifier = XaiNatsNotifier::new(nats.clone(), xai_prefix.clone());
            let xai_agent = XaiAgent::with_deps(xai_notifier, "grok-4", "", MockXaiHttpClient::default());
            let (_, xai_io) = AgentSideNatsConnection::new(xai_agent, nats.clone(), xai_prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(async move {
                xai_io.await.ok();
            });

            // ── 2. Attach OpenRouterAgent to "acp.or" ─────────────────────────
            let or_prefix = AcpPrefix::new("acp.or").unwrap();
            let or_notifier = OrNatsNotifier::new(nats.clone(), or_prefix.clone());
            let or_agent = OpenRouterAgent::with_deps(
                or_notifier,
                "anthropic/claude-3-5-sonnet",
                "",
                MockOpenRouterHttpClient::default(),
            );
            let (_, or_io) = AgentSideNatsConnection::new(or_agent, nats.clone(), or_prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(async move {
                or_io.await.ok();
            });

            // Allow both agents to subscribe before sending requests.
            tokio::time::sleep(Duration::from_millis(300)).await;

            // ── 3. Create a session in XAI and import messages into it ────────
            let new_resp_msg = tokio::time::timeout(
                Duration::from_secs(10),
                nats.request(
                    "acp.xai.agent.session.new",
                    serde_json::to_vec(&serde_json::json!({
                        "sessionId": null,
                        "cwd": "/tmp",
                        "mcpServers": []
                    }))
                    .unwrap()
                    .into(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            let new_resp: serde_json::Value = serde_json::from_slice(&new_resp_msg.payload).unwrap();
            let xai_session_id = new_resp["sessionId"].as_str().unwrap().to_string();

            tokio::time::timeout(
                Duration::from_secs(10),
                nats.request(
                    "acp.xai.agent.ext.session/import",
                    serde_json::to_vec(&serde_json::json!({
                        "sessionId": xai_session_id,
                        "messages": [
                            {"role": "user",      "text": "question from xai"},
                            {"role": "assistant", "text": "answer from xai"}
                        ]
                    }))
                    .unwrap()
                    .into(),
                ),
            )
            .await
            .unwrap()
            .unwrap();

            // ── 4. Register both runners in the registry ───────────────────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut xai_cap = AgentCapability::new("xai", ["chat", "explore", "plan"], "acp.xai.agent.>");
            xai_cap.metadata = serde_json::json!({ "models": ["grok-4"], "acp_prefix": "acp.xai" });
            registry.register(&xai_cap).await.unwrap();

            let mut or_cap = AgentCapability::new("openrouter", ["chat", "explore", "plan"], "acp.or.agent.>");
            or_cap.metadata = serde_json::json!({
                "models": ["anthropic/claude-3-5-sonnet"],
                "acp_prefix": "acp.or"
            });
            registry.register(&or_cap).await.unwrap();

            // ── 5. Migrate XAI session to OpenRouter via CrossRunnerSwitcher ───
            let mut switcher =
                CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry);
            let trogon_cli::cross_runner::SwitchSurface { new_prefix, new_session_id, .. } = switcher
                .switch_model(
                    "acp.xai",
                    &xai_session_id,
                    "grok-4",
                    "anthropic/claude-3-5-sonnet",
                    "/tmp",
                )
                .await
                .expect("switch_model from xai to openrouter must succeed");

            assert_eq!(new_prefix, "acp.or", "target prefix must be acp.or");
            assert!(!new_session_id.is_empty(), "new_session_id must not be empty");

            // ── 6. Verify messages in the OR session via session/export ─────────
            let export_msg = tokio::time::timeout(
                Duration::from_secs(10),
                nats.request(
                    "acp.or.agent.ext.session/export",
                    serde_json::to_vec(&serde_json::json!({ "sessionId": new_session_id }))
                        .unwrap()
                        .into(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            // `session/export` returns the portable session in one of two shapes
            // (trogon-runner-tools::portable_session): V1 is a bare array of
            // `{role, text}`; V2 is a versioned object `{version:2, messages:[{role,
            // blocks:[{type:"text", text}]}]}` used once any block is richer than plain
            // text. Normalize both to `(role, text)` so the assertion is format-agnostic.
            // The raw ext-method NATS reply wraps the export in `{"result": <export>}`;
            // unwrap it first. The inner export is then either V1 (a bare `{role, text}`
            // array) or V2 (a versioned `{version:2, messages:[...]}` object).
            let envelope: serde_json::Value = serde_json::from_slice(&export_msg.payload).unwrap();
            let export = envelope.get("result").cloned().unwrap_or(envelope);
            let normalized: Vec<(String, String)> = match &export {
                serde_json::Value::Array(items) => items
                    .iter()
                    .map(|m| {
                        (
                            m["role"].as_str().unwrap_or_default().to_string(),
                            m["text"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
                serde_json::Value::Object(_) => export["messages"]
                    .as_array()
                    .expect("V2 export must carry a messages array")
                    .iter()
                    .map(|m| {
                        let text = m["blocks"]
                            .as_array()
                            .and_then(|blocks| {
                                blocks.iter().find_map(|b| {
                                    (b["type"] == "text").then(|| b["text"].as_str().unwrap_or_default().to_string())
                                })
                            })
                            .unwrap_or_default();
                        (m["role"].as_str().unwrap_or_default().to_string(), text)
                    })
                    .collect(),
                other => panic!("unexpected export shape: {other:?}"),
            };

            assert_eq!(
                normalized.len(),
                2,
                "migrated OR session must have 2 messages; got: {normalized:?}"
            );
            assert_eq!(normalized[0].0, "user", "first migrated message must be user");
            assert_eq!(
                normalized[0].1, "question from xai",
                "first message text must survive migration"
            );
            assert_eq!(normalized[1].0, "assistant", "second migrated message must be assistant");
            assert_eq!(
                normalized[1].1, "answer from xai",
                "second message text must survive migration"
            );
        })
        .await;
}

/// The cross-runner `switch_model` migration preserves `ToolUse` / `ToolResult`
/// blocks losslessly through the `PortableMessageV2` format
/// (trogon-runner-tools::portable_session): tool_use carries the FULL structured
/// `input` (a JSON object, untruncated) and `parent_tool_use_id`, per
/// cambio-modelo.md §11 (No-Lossy: "input JSON completo ... parent_tool_use_id")
/// and §637 ("un modelo con tools recibe tool calls estructurados").
///
/// This previously LOCKED IN the old lossy behavior (input stringified + truncated
/// to 240 chars, parent dropped); it now asserts exact preservation after the format
/// was made lossless. `input_summary` is still emitted for N-1 readers, but the
/// structured `input` is the source of truth on import.
#[tokio::test]
async fn switch_model_tool_blocks_are_preserved_structurally() {
    let (_c, port) = start_nats_js().await;
    let (nats, js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Seed source session with a tool-call round-trip ────────────
            let store = NatsSessionStore::open(&js).await.unwrap();

            // Small structured input — preserved exactly as an object.
            let small_input = serde_json::json!({ "command": "ls -la", "timeout_ms": 5000 });
            // Large structured input (its string form exceeds the 240-char summary cap) —
            // still preserved in full, NOT truncated, because the structured `input` is carried.
            let big_value = "x".repeat(400);
            let large_input = serde_json::json!({ "path": "/etc/config", "blob": big_value });

            let src_state = SessionState {
                messages: vec![
                    AgentMessage::user_text("list the files in this repo"),
                    AgentMessage::assistant(vec![
                        AgentContentBlock::Text {
                            text: "Let me list them.".into(),
                        },
                        AgentContentBlock::ToolUse {
                            id: "toolu_abc123".into(),
                            name: "bash".into(),
                            input: small_input.clone(),
                            parent_tool_use_id: Some("toolu_parent".into()),
                        },
                        AgentContentBlock::ToolUse {
                            id: "toolu_big".into(),
                            name: "write".into(),
                            input: large_input.clone(),
                            parent_tool_use_id: None,
                        },
                    ]),
                    AgentMessage {
                        role: "user".into(),
                        content: vec![AgentContentBlock::ToolResult {
                            tool_use_id: "toolu_abc123".into(),
                            content: "total 8\n-rw-r--r-- 1 user user 0 main.rs".into(),
                            blocks: vec![],
                        }],
                    },
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "There is one file: main.rs".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("switcher-tool-1", &src_state).await.unwrap();

            // ── 2. Start two TrogonAgent instances on different prefixes ───────
            attach_agent(make_agent(store.clone(), "acp.src"), nats.clone(), "acp.src");
            attach_agent(make_agent(store.clone(), "acp.dst"), nats.clone(), "acp.dst");
            tokio::time::sleep(Duration::from_millis(60)).await;

            // ── 3. Registry: "dst-model" → "acp.dst" ──────────────────────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut dst_cap = AgentCapability::new("dst-runner", ["chat"], "agents.dst.>");
            dst_cap.metadata = serde_json::json!({ "models": ["dst-model"], "acp_prefix": "acp.dst" });
            registry.register(&dst_cap).await.unwrap();

            let mut switcher = CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry);

            // ── 4. Migrate the session ────────────────────────────────────────
            let trogon_cli::cross_runner::SwitchSurface { new_prefix, new_session_id, .. } = switcher
                .switch_model("acp.src", "switcher-tool-1", "sonnet", "dst-model", "/tmp")
                .await
                .expect("switch_model should succeed");
            assert_eq!(new_prefix, "acp.dst");

            // ── 5. Characterize what survived vs what was lost ────────────────
            let dst = store.load(&new_session_id).await.unwrap();

            assert_eq!(dst.messages.len(), 4, "message count is preserved");

            // msg[0]: plain user text — fully preserved.
            assert!(matches!(
                &dst.messages[0].content[0],
                AgentContentBlock::Text { text } if text == "list the files in this repo"
            ));

            // msg[1]: assistant text preserved; tool_use id/name preserved.
            assert!(matches!(
                &dst.messages[1].content[0],
                AgentContentBlock::Text { text } if text == "Let me list them."
            ));

            // First tool_use: id + name + FULL structured input + parent linkage preserved.
            match &dst.messages[1].content[1] {
                AgentContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    parent_tool_use_id,
                } => {
                    assert_eq!(id, "toolu_abc123", "tool_use id is preserved");
                    assert_eq!(name, "bash", "tool_use name is preserved");
                    // Structured object preserved exactly — not collapsed to a string.
                    assert_eq!(input, &small_input, "tool_use input is preserved as a structured object");
                    assert!(input.is_object(), "input remains a JSON object");
                    // Parent linkage preserved (was Some).
                    assert_eq!(
                        parent_tool_use_id.as_deref(),
                        Some("toolu_parent"),
                        "parent_tool_use_id is preserved across migration"
                    );
                }
                other => panic!("expected ToolUse, got {other:?}"),
            }

            // Second tool_use: large input preserved in FULL (not truncated), as an object.
            match &dst.messages[1].content[2] {
                AgentContentBlock::ToolUse { id, input, .. } => {
                    assert_eq!(id, "toolu_big");
                    assert_eq!(input, &large_input, "large structured input is preserved untruncated");
                    assert!(input.is_object(), "large input remains a JSON object");
                }
                other => panic!("expected ToolUse, got {other:?}"),
            }

            // msg[2]: ToolResult — id and (short) content preserved.
            match &dst.messages[2].content[0] {
                AgentContentBlock::ToolResult { tool_use_id, content, .. } => {
                    assert_eq!(tool_use_id, "toolu_abc123", "tool_result id is preserved");
                    assert_eq!(
                        content, "total 8\n-rw-r--r-- 1 user user 0 main.rs",
                        "short tool_result content is preserved (would truncate at 500)"
                    );
                }
                other => panic!("expected ToolResult, got {other:?}"),
            }

            // msg[3]: final assistant text — fully preserved.
            assert!(matches!(
                &dst.messages[3].content[0],
                AgentContentBlock::Text { text } if text == "There is one file: main.rs"
            ));
        })
        .await;
}

/// After `switch_model` migrates a session, the new runner can immediately
/// accept and process a prompt on the new prefix+session_id.
///
/// Verifies the full post-switch flow: export → new_session → import → prompt
/// → end_turn on the migrated session, exercising NATS routing for the new
/// prefix end-to-end.
#[tokio::test]
async fn switch_model_prompt_on_new_runner_after_migration() {
    let (_c, port) = start_nats_js().await;
    let (nats, js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Seed source session ─────────────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let src_state = SessionState {
                messages: vec![
                    AgentMessage::user_text("first question"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "first answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("post-switch-src-1", &src_state).await.unwrap();

            // ── 2. Start two TrogonAgent instances ─────────────────────────────
            attach_agent(make_agent(store.clone(), "acp.ps.src"), nats.clone(), "acp.ps.src");
            attach_agent(make_agent(store.clone(), "acp.ps.dst"), nats.clone(), "acp.ps.dst");
            tokio::time::sleep(Duration::from_millis(80)).await;

            // ── 3. Register the destination runner ────────────────────────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut dst_cap = AgentCapability::new("ps-dst-runner", ["chat"], "acp.ps.dst.agent.>");
            dst_cap.metadata = serde_json::json!({ "models": ["ps-model"], "acp_prefix": "acp.ps.dst" });
            registry.register(&dst_cap).await.unwrap();

            // ── 4. Migrate the session ────────────────────────────────────────
            let mut switcher = CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry);
            let trogon_cli::cross_runner::SwitchSurface { new_prefix, new_session_id, .. } = switcher
                .switch_model("acp.ps.src", "post-switch-src-1", "sonnet", "ps-model", "/tmp")
                .await
                .expect("switch_model must succeed");

            assert_eq!(new_prefix, "acp.ps.dst");
            assert!(!new_session_id.is_empty());

            // ── 5. Send a prompt to the migrated session ──────────────────────
            let prompt_subject = format!("{new_prefix}.session.{new_session_id}.agent.prompt");
            let body = serde_json::to_vec(&serde_json::json!({
                "sessionId": new_session_id,
                "prompt": [{"type": "text", "text": "continue the conversation"}]
            }))
            .unwrap();

            let resp_msg = tokio::time::timeout(Duration::from_secs(15), nats.request(prompt_subject, body.into()))
                .await
                .expect("timed out waiting for prompt response on new runner")
                .expect("NATS request failed");

            let resp: serde_json::Value = serde_json::from_slice(&resp_msg.payload).unwrap();

            assert_eq!(
                resp["stopReason"].as_str(),
                Some("end_turn"),
                "prompt to migrated session must complete with end_turn; got: {resp}"
            );
        })
        .await;
}

/// Canonical-path switch (Fase 8 / 11): with the Session Kernel enabled and
/// `runner_binding_mode=canonical`, `CrossRunnerSwitcher::switch_model` drives the
/// canonical orchestration (`switch_via_session_kernel`) instead of plain handoff. This
/// exercises the canonical path end-to-end over REAL NATS JetStream between two real
/// runners — coverage the mock-only `complex_session_fixture` cannot provide.
///
/// It verifies (a) the switch completes and the session is migrated to the target
/// runner, and (b) the kernel materialized a canonical snapshot for the session over
/// real NATS (shadow event log → snapshot), proving the canonical kernel integration
/// ran rather than being bypassed.
///
/// Requires Docker.
#[tokio::test]
async fn canonical_switch_records_kernel_state_over_real_nats() {
    let (_c, port) = start_nats_js_v211().await;
    let (nats, js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Seed source session ────────────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let src_state = SessionState {
                messages: vec![
                    AgentMessage::user_text("canonical question"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "canonical answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("sess_canon_1", &src_state).await.unwrap();

            // ── 2. Two real runners on different prefixes ─────────────────────
            attach_agent(make_agent(store.clone(), "acp.src"), nats.clone(), "acp.src");
            attach_agent(make_agent(store.clone(), "acp.dst"), nats.clone(), "acp.dst");
            tokio::time::sleep(Duration::from_millis(60)).await;

            // ── 3. Registry: "dst-model" → "acp.dst" ──────────────────────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut dst_cap = AgentCapability::new("dst-runner", ["chat"], "agents.dst.>");
            dst_cap.metadata = serde_json::json!({ "models": ["dst-model"], "acp_prefix": "acp.dst" });
            registry.register(&dst_cap).await.unwrap();

            // ── 4. Session Kernel in CANONICAL binding mode ───────────────────
            let mut flags = SessionKernelFeatureFlags::default().with_runner_binding_mode("canonical");
            flags.session_kernel_enabled = true;
            assert!(flags.use_canonical_runner_binding(), "test must drive the canonical path");
            let stack = SessionKernelStack::provision(nats.clone(), flags, SessionKernelOperationalPolicy::default())
                .await
                .expect("kernel stack must provision against real NATS");
            // Keep a snapshot-store handle to inspect canonical state after the switch.
            let snapshots = stack.snapshots.clone();

            let mut switcher =
                CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry).with_kernel_stack(stack);

            // ── 5. Switch (canonical path; falls back to handoff only if the gate
            //         blocks — either way the kernel records canonical state first) ──
            let trogon_cli::cross_runner::SwitchSurface { new_prefix, new_session_id, .. } = switcher
                .switch_model("acp.src", "sess_canon_1", "sonnet", "dst-model", "/tmp")
                .await
                .expect("switch_model should complete with the kernel enabled");
            assert_eq!(new_prefix, "acp.dst");
            assert!(!new_session_id.is_empty());

            // ── 6. Session migrated to the destination runner ─────────────────
            let dst_state = store.load(&new_session_id).await.unwrap();
            assert_eq!(dst_state.messages.len(), 2, "migrated session must have 2 messages");
            assert_eq!(dst_state.messages[0].role, "user");
            assert_eq!(dst_state.messages[1].role, "assistant");

            // ── 7. Canonical kernel materialized a snapshot over real NATS ────
            let sid = SessionId::new("sess_canon_1").unwrap();
            let snapshot = snapshots.load_snapshot(&sid).await.expect("snapshot load must not error");
            assert!(
                snapshot.is_some(),
                "canonical kernel must materialize a snapshot for the switched session"
            );
        })
        .await;
}
