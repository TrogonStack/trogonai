//! Integration tests verifying that `XaiAgent` persists sessions to real NATS JetStream.
//!
//! These tests spin up a fresh NATS container and verify that `new_session`,
//! `fork_session`, `close_session`, `prompt`, and the `with_loaders` /
//! `TENANT_ID` wiring all write the expected data to the SESSIONS KV bucket.
//! The xAI HTTP client and session notifier are no-op stubs — no real network needed.

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::{
    Agent as _, CancelNotification, CloseSessionRequest, ContentBlock, ForkSessionRequest, NewSessionRequest,
    PromptRequest, SessionNotification, SetSessionModelRequest,
};
use async_nats::jetstream;
use async_trait::async_trait;
use futures_util::stream::{self, LocalBoxStream, StreamExt as _};
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_xai_runner::{
    AgentLoader, InputItem, SessionNotifier, SkillLoader, XaiAgent, XaiEvent, XaiHttpClient,
    session_store::NatsSessionStore,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn make_js() -> (jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("start NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    (jetstream::new(nats), container)
}

// ── Inline stubs ──────────────────────────────────────────────────────────────

struct NoOpNotifier;

#[async_trait(?Send)]
impl SessionNotifier for NoOpNotifier {
    async fn notify(&self, _: SessionNotification) {}
}

struct NoOpHttpClient;

#[async_trait(?Send)]
impl XaiHttpClient for NoOpHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[trogon_xai_runner::ToolSpec],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        Box::pin(stream::empty())
    }
}

/// Returns a fixed assistant reply with token usage then ends.
struct ReplyWithUsageHttpClient;

#[async_trait(?Send)]
impl XaiHttpClient for ReplyWithUsageHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[trogon_xai_runner::ToolSpec],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        Box::pin(stream::iter(vec![
            XaiEvent::Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                cached_tokens: 0,
            },
            XaiEvent::TextDelta {
                text: "Hello with usage!".to_string(),
            },
        ]))
    }
}

/// Returns a fixed assistant reply ("Hello back!") then ends.
struct ReplyHttpClient;

#[async_trait(?Send)]
impl XaiHttpClient for ReplyHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[trogon_xai_runner::ToolSpec],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        Box::pin(stream::iter(vec![XaiEvent::TextDelta {
            text: "Hello back!".to_string(),
        }]))
    }
}

fn make_agent(store: NatsSessionStore) -> XaiAgent<NoOpHttpClient, NoOpNotifier> {
    XaiAgent::with_deps(NoOpNotifier, "grok-4", "", NoOpHttpClient).with_session_store(Arc::new(store))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn agent_new_session_persists_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let key = format!("default.{session_id}");
            let bytes = kv.get(&key).await.unwrap().expect("entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(v["id"], session_id.as_str());
            assert_eq!(v["tenant_id"], "default");
            assert!(v["created_at"].as_str().is_some());
            assert!(v["updated_at"].as_str().is_some());
            assert_eq!(v["messages"].as_array().unwrap().len(), 0);
        })
        .await;
}

#[tokio::test]
async fn agent_new_session_name_defaults_to_new_conversation() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv.get(&format!("default.{session_id}")).await.unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(v["name"], "New Conversation");
        })
        .await;
}

#[tokio::test]
async fn agent_new_session_model_defaults_to_agent_default() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv.get(&format!("default.{session_id}")).await.unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(v["model"], "grok-4");
        })
        .await;
}

#[tokio::test]
async fn agent_fork_session_creates_separate_nats_entry() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let new_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let source_id = new_resp.session_id.clone();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(source_id.clone(), PathBuf::from("/tmp")))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");

            let source_key = format!("default.{}", source_id);
            assert!(kv.get(&source_key).await.unwrap().is_some(), "source entry must exist");

            let fork_key = format!("default.{fork_id}");
            let bytes = kv.get(&fork_key).await.unwrap().expect("fork entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["id"], fork_id.as_str());
            assert_ne!(fork_id, source_id.to_string(), "fork must have a distinct id");
        })
        .await;
}

#[tokio::test]
async fn agent_close_session_saves_final_snapshot_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .close_session(CloseSessionRequest::new(resp.session_id))
                .await
                .unwrap();

            // close_session calls store.save — snapshot must survive in NATS.
            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let key = format!("default.{session_id}");
            assert!(
                kv.get(&key).await.unwrap().is_some(),
                "snapshot must remain in NATS after close"
            );
        })
        .await;
}

// ── prompt → store.save ───────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_persists_user_message_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    // Requires a non-empty API key so prompt does not reject before calling the client.
    let agent =
        XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", NoOpHttpClient).with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("Hello from integration test".to_string())],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist after prompt");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            // The user turn must be recorded in the snapshot messages.
            let messages = v["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 1, "one user message expected");
            assert_eq!(messages[0]["role"], "user");
            assert_eq!(messages[0]["content"][0]["text"], "Hello from integration test");
        })
        .await;
}

// ── with_loaders + new_session writes console model to SESSIONS ───────────────

async fn kv_put(js: &jetstream::Context, bucket: &str, key: &str, json: &str) {
    let kv = js
        .create_or_update_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket.to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("open KV bucket");
    kv.put(key, bytes::Bytes::from(json.to_string())).await.expect("KV put");
}

#[tokio::test]
async fn new_session_with_loaders_writes_console_model_to_sessions() {
    let (js, _c) = make_js().await;

    // Seed console KV with agent config that overrides the default model.
    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-x",
        r#"{"skill_ids":[],"model":{"id":"grok-3-mini"}}"#,
    )
    .await;

    let agent_loader = AgentLoader::open(&js).await.expect("AgentLoader");
    let skill_loader = SkillLoader::open(&js).await.expect("SkillLoader");
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "", NoOpHttpClient)
        .with_loaders("agent-x", Arc::new(agent_loader), Arc::new(skill_loader))
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            // Model must come from CONSOLE_AGENTS, not the agent default ("grok-4").
            assert_eq!(v["model"], "grok-3-mini");
        })
        .await;
}

// ── TENANT_ID env var → KV key prefix ─────────────────────────────────────────

#[tokio::test]
async fn tenant_id_env_var_sets_kv_key_prefix() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    // Set TENANT_ID before constructing the agent (read in with_deps).
    // SAFETY: single-threaded test body; no other thread reads this var concurrently.
    unsafe {
        std::env::set_var("TENANT_ID", "acme-corp");
    }
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "", NoOpHttpClient).with_session_store(Arc::new(store));
    unsafe {
        std::env::remove_var("TENANT_ID");
    }

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");

            // Entry must be stored under the custom tenant prefix.
            let custom_key = format!("acme-corp.{session_id}");
            let bytes = kv
                .get(&custom_key)
                .await
                .unwrap()
                .expect("entry must exist under custom tenant prefix");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["tenant_id"], "acme-corp");

            // Must NOT exist under the default prefix.
            let default_key = format!("default.{session_id}");
            assert!(
                kv.get(&default_key).await.unwrap().is_none(),
                "entry must not exist under 'default' prefix"
            );
        })
        .await;
}

// ── session branching persistence ─────────────────────────────────────────────

/// fork_session persists parent_session_id to the SESSIONS KV bucket.
#[tokio::test]
async fn fork_session_persists_parent_session_id_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let src_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/src")))
                .await
                .unwrap();
            let src_id = src_resp.session_id.clone();

            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), PathBuf::from("/fork")))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{fork_id}"))
                .await
                .unwrap()
                .expect("fork KV entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                v["parent_session_id"].as_str(),
                Some(src_id.to_string().as_str()),
                "fork KV entry must record parent_session_id"
            );
        })
        .await;
}

/// fork_session with branchAtIndex persists branched_at_index to the SESSIONS KV bucket.
#[tokio::test]
async fn fork_with_branch_at_index_persists_branched_at_index_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let src_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/src")))
                .await
                .unwrap();
            let src_id = src_resp.session_id.clone();

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 0 }),
            )
            .unwrap();
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), PathBuf::from("/fork")).meta(meta))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{fork_id}"))
                .await
                .unwrap()
                .expect("fork KV entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                v["branched_at_index"],
                serde_json::json!(0),
                "fork KV entry must record branched_at_index"
            );
        })
        .await;
}

// ── set_session_model → updated model persisted to SESSIONS ──────────────────

#[tokio::test]
async fn set_session_model_reflected_in_sessions_kv_after_close() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            let session_id_val = resp.session_id.clone();

            // Change model then force a save via close_session.
            agent
                .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
                .await
                .unwrap();
            agent
                .close_session(CloseSessionRequest::new(session_id_val))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist after close");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                v["model"], "grok-3-mini",
                "SESSIONS KV model must reflect set_session_model change"
            );
        })
        .await;
}

// ── session name derived from first user message ──────────────────────────────

#[tokio::test]
async fn session_name_derived_from_first_user_message_after_prompt() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent =
        XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyHttpClient).with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("My first message".to_string())],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist after prompt");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                v["name"], "My first message",
                "session name must be set to the first user message text after prompt"
            );
        })
        .await;
}

// ── prompt with assistant response persists both messages ─────────────────────

#[tokio::test]
async fn agent_id_written_to_sessions_kv_when_with_loaders() {
    let (js, _c) = make_js().await;

    let agent_loader = AgentLoader::open(&js).await.expect("AgentLoader");
    let skill_loader = SkillLoader::open(&js).await.expect("SkillLoader");
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "", NoOpHttpClient)
        .with_loaders("my-agent-id", Arc::new(agent_loader), Arc::new(skill_loader))
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(
                v["agent_id"], "my-agent-id",
                "agent_id must be written to SESSIONS KV; got: {v}"
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_with_assistant_response_persists_both_messages_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent =
        XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyHttpClient).with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("Hi".to_string())],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            let messages = v["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 2, "user + assistant messages expected");
            assert_eq!(messages[0]["role"], "user");
            assert_eq!(messages[0]["content"][0]["text"], "Hi");
            assert_eq!(messages[1]["role"], "assistant");
            assert_eq!(messages[1]["content"][0]["text"], "Hello back!");
        })
        .await;
}

// ── token usage persisted to SESSIONS KV ─────────────────────────────────────

/// When `XaiEvent::Usage` fires during a prompt, the assistant message stored
/// in SESSIONS KV must carry `input_tokens` and `output_tokens`.
#[tokio::test]
async fn prompt_token_usage_persisted_to_sessions_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyWithUsageHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("count my tokens")],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            let messages = v["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 2, "user + assistant messages expected");
            let usage = &messages[1]["usage"];
            assert_eq!(
                usage["input_tokens"], 10,
                "input_tokens must match prompt_tokens from Usage event; got: {usage}"
            );
            assert_eq!(
                usage["output_tokens"], 5,
                "output_tokens must match completion_tokens from Usage event; got: {usage}"
            );
        })
        .await;
}

// ── session name truncation ───────────────────────────────────────────────────

/// When the first user message exceeds 60 characters, `build_snapshot` must
/// truncate the session name to 60 chars and append "…".
#[tokio::test]
async fn session_name_truncated_to_60_chars_in_sessions_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent =
        XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyHttpClient).with_session_store(Arc::new(store));

    // 80-character message — well over the 60-char limit.
    let long_message = "A".repeat(80);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from(long_message.clone())],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist after prompt");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let name = v["name"].as_str().expect("name must be a string");

            assert!(name.ends_with('…'), "truncated name must end with '…'; got: {name:?}");
            // 60 ASCII chars + the 3-byte UTF-8 ellipsis = 63 bytes, 61 chars.
            assert_eq!(
                name.chars().count(),
                61,
                "truncated name must be 60 chars + '…'; got: {name:?}"
            );
        })
        .await;
}

// ── PR 15: _meta.systemPrompt stored in session state ────────────────────────

/// Verifies that _meta.systemPrompt set on new_session is stored in the
/// in-memory session and readable via ext session/get_state.
#[tokio::test]
async fn new_session_meta_system_prompt_stored_in_session_state() {
    use agent_client_protocol::ExtRequest;

    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = make_agent(store);

    tokio::task::LocalSet::new()
        .run_until(async move {
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "systemPrompt": "act like a pirate" }),
            )
            .unwrap();
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")).meta(meta))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let raw_params =
                serde_json::value::RawValue::from_string(serde_json::json!({ "sessionId": session_id }).to_string())
                    .unwrap();
            let ext_resp = agent
                .ext_method(ExtRequest::new("session/get_state", raw_params.into()))
                .await
                .unwrap();
            let state: serde_json::Value = serde_json::from_str(ext_resp.0.get()).unwrap();
            assert_eq!(
                state["system_prompt"].as_str(),
                Some("act like a pirate"),
                "_meta.systemPrompt must be stored in the session state"
            );
        })
        .await;
}

// ── token tracking persisted to KV ───────────────────────────────────────────

/// After a prompt with token usage, `totalInputTokens` and `totalOutputTokens`
/// must appear in the SESSIONS KV snapshot.
#[tokio::test]
async fn token_totals_persisted_to_sessions_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyWithUsageHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("track my tokens")],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("snapshot must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(
                v["total_input_tokens"], 10,
                "total_input_tokens must be 10 after prompt"
            );
            assert_eq!(
                v["total_output_tokens"], 5,
                "total_output_tokens must be 5 after prompt"
            );
        })
        .await;
}

/// After forking a session that has token totals, the fork's KV snapshot must
/// not carry the parent's totals (zero values are omitted from JSON).
#[tokio::test]
async fn fork_session_token_totals_absent_in_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyWithUsageHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let src = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let src_id = src.session_id.to_string();

            agent
                .prompt(PromptRequest::new(src.session_id, vec![ContentBlock::from("prompt")]))
                .await
                .unwrap();

            let fork = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), PathBuf::from("/fork")))
                .await
                .unwrap();
            let fork_id = fork.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{fork_id}"))
                .await
                .unwrap()
                .expect("fork snapshot must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert!(
                v.get("total_input_tokens").is_none(),
                "fork must not inherit parent's total_input_tokens; got: {v}"
            );
            assert!(
                v.get("total_output_tokens").is_none(),
                "fork must not inherit parent's total_output_tokens; got: {v}"
            );
        })
        .await;
}

// ── cache_read_tokens in KV ───────────────────────────────────────────────────

/// HTTP client that returns a Usage event with non-zero cached_tokens.
struct CacheReadUsageHttpClient;

#[async_trait(?Send)]
impl XaiHttpClient for CacheReadUsageHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[trogon_xai_runner::ToolSpec],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        Box::pin(stream::iter(vec![
            XaiEvent::Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                cached_tokens: 50,
            },
            XaiEvent::TextDelta {
                text: "cached reply".to_string(),
            },
        ]))
    }
}

/// After a prompt where the xAI Usage event has `cached_tokens > 0`,
/// `total_cache_read_tokens` must be persisted to the SESSIONS KV bucket.
#[tokio::test]
async fn cache_read_tokens_persisted_to_sessions_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", CacheReadUsageHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("use cache")],
                ))
                .await
                .unwrap();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("snapshot must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(
                v["total_cache_read_tokens"], 50,
                "total_cache_read_tokens must be 50 when cached_tokens=50; got: {v}"
            );
        })
        .await;
}

// ── cancel path persists tokens to KV ────────────────────────────────────────

/// HTTP client that emits Usage + partial text then blocks forever.
/// Signals `ready` when the stream is set up so the test can time the cancel.
struct SlowCancelHttpClient {
    ready: Arc<tokio::sync::Notify>,
}

#[async_trait(?Send)]
impl XaiHttpClient for SlowCancelHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[trogon_xai_runner::ToolSpec],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        self.ready.notify_one();
        let initial = stream::iter(vec![
            XaiEvent::Usage {
                prompt_tokens: 20,
                completion_tokens: 8,
                cached_tokens: 15,
            },
            XaiEvent::TextDelta {
                text: "partial".to_string(),
            },
        ]);
        Box::pin(initial.chain(stream::pending()))
    }
}

/// When a prompt is cancelled after the xAI Usage event has been received
/// (but before the stream ends), the cancel path must persist the billed
/// tokens to the SESSIONS KV bucket.
#[tokio::test]
async fn cancel_prompt_token_totals_persisted_to_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let ready = Arc::new(tokio::sync::Notify::new());
    let agent = Arc::new(
        XaiAgent::with_deps(
            NoOpNotifier,
            "grok-4",
            "dummy-key",
            SlowCancelHttpClient {
                ready: Arc::clone(&ready),
            },
        )
        .with_session_store(Arc::new(store)),
    );

    tokio::task::LocalSet::new()
        .run_until(async move {
            let session_id = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap()
                .session_id
                .to_string();

            let agent_prompt = Arc::clone(&agent);
            let sid_for_prompt = session_id.clone();
            let prompt_handle = tokio::task::spawn_local(async move {
                agent_prompt
                    .prompt(PromptRequest::new(
                        sid_for_prompt,
                        vec![ContentBlock::from("partial call")],
                    ))
                    .await
                    .unwrap()
            });

            // `chat_stream` was called → Usage event already in the stream buffer,
            // cancel channel is registered. Sending cancel now will interrupt after
            // the Usage event has been processed.
            ready.notified().await;
            agent.cancel(CancelNotification::new(session_id.clone())).await.unwrap();

            let result = prompt_handle.await.unwrap();
            assert_eq!(
                result.stop_reason,
                agent_client_protocol::StopReason::Cancelled,
                "stop_reason must be Cancelled"
            );

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("snapshot must exist after cancel with billed tokens");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(
                v["total_input_tokens"], 20,
                "cancel path must persist input tokens to KV; got: {v}"
            );
            assert_eq!(
                v["total_output_tokens"], 8,
                "cancel path must persist output tokens to KV; got: {v}"
            );
            assert_eq!(
                v["total_cache_read_tokens"], 15,
                "cancel path must persist cache_read tokens to KV; got: {v}"
            );
        })
        .await;
}

// ── Context compaction end-to-end (runner → compactor over NATS) ─────────────────

/// Drives a prompt whose history exceeds 85 % of the model's context window, with
/// a stand-in compactor responder on `trogon.compactor.compact`. Verifies the
/// runner: (1) detects the threshold, (2) calls the compactor over NATS, (3)
/// applies the compacted history, and (4) clears `last_response_id` (mandatory
/// for xAI's stateful API so the next turn re-sends the compacted history).
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn prompt_compacts_via_nats_and_clears_last_response_id() {
    use std::time::Duration;
    use trogon_xai_runner::Message;

    // Raw NATS client (needed for compactor_nats + the responder) + JetStream store.
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("start NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = jetstream::new(nats.clone());
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    // Stand-in compactor: replies with a 3-message compacted history.
    let responder = nats.clone();
    tokio::spawn(async move {
        let mut sub = responder.subscribe("trogon.compactor.compact").await.unwrap();
        while let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                let body = serde_json::json!({
                    "messages": [
                        {"role": "user", "content": [{"type": "text", "text": "<context-summary>\nsummary\n</context-summary>"}]},
                        {"role": "assistant", "content": [{"type": "text", "text": "ack"}]},
                        {"role": "user", "content": [{"type": "text", "text": "recent"}]}
                    ],
                    "compacted": true,
                    "tokens_before": 999_999,
                    "tokens_after": 100,
                    "kept_count": 1
                });
                responder
                    .publish(reply, serde_json::to_vec(&body).unwrap().into())
                    .await
                    .ok();
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // grok-3 → 131_072 token window; 85 % ≈ 111_411 tokens.
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-3", "test-key", ReplyHttpClient)
        .with_session_store(Arc::new(store))
        .with_compactor(nats.clone());

    // Insert a session whose history is well over the threshold (25 × 30 KB ≈
    // 750 KB ≈ 187k tokens; even after trim_history to the recent window it stays
    // over 111k). Pre-set last_response_id so we can prove it gets cleared.
    let big = "x".repeat(30_000);
    let mut history = Vec::new();
    for i in 0..25 {
        if i % 2 == 0 {
            history.push(Message::user(format!("{big}{i}")));
        } else {
            history.push(Message::assistant_text(format!("{big}{i}")));
        }
    }
    agent
        .test_insert_session_with_response_id("s-compact", "/tmp", None, Some("resp-old".to_string()))
        .await;
    agent.test_set_session_history("s-compact", history).await;
    assert_eq!(
        agent.test_last_response_id("s-compact").await,
        Some("resp-old".to_string()),
        "precondition: last_response_id is set before the turn"
    );

    tokio::task::LocalSet::new()
        .run_until(async move {
            agent
                .prompt(PromptRequest::new(
                    SessionId::from("s-compact"),
                    vec![ContentBlock::from("hi".to_string())],
                ))
                .await
                .unwrap();

            // The compactor's 3-message result replaced the large history.
            let hist = agent.test_session_history("s-compact").await;
            assert_eq!(
                hist.len(),
                3,
                "history must be replaced by the compactor's compacted result"
            );
            assert!(
                hist[0].content.as_deref().unwrap_or("").contains("context-summary"),
                "first message must be the summary checkpoint"
            );
            // Mandatory: last_response_id cleared so the next turn re-sends the
            // full compacted history to xAI.
            assert_eq!(
                agent.test_last_response_id("s-compact").await,
                None,
                "last_response_id must be cleared after compaction"
            );
        })
        .await;
}

// ── Permission emit e2e: xai emits request_permission on Ask and gates on the reply ──

/// Notifier that records every notification so the test can assert the deny update.
struct RecordingNotifier {
    notes: Arc<std::sync::Mutex<Vec<SessionNotification>>>,
}

#[async_trait(?Send)]
impl SessionNotifier for RecordingNotifier {
    async fn notify(&self, n: SessionNotification) {
        self.notes.lock().unwrap().push(n);
    }
}

/// On the first turn asks to call the client-side `read_file` tool on a protected
/// path (`*.key`), which forces the permission prompt in every mode (read-only
/// tools are otherwise auto-allowed); ends on the continuation turn.
struct ReadFileToolThenDone {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait(?Send)]
impl XaiHttpClient for ReadFileToolThenDone {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[trogon_xai_runner::ToolSpec],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Box::pin(stream::iter(vec![
                XaiEvent::ResponseId { id: "r1".to_string() },
                XaiEvent::FunctionCall {
                    call_id: "c1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"secret.key"}"#.to_string(),
                },
                XaiEvent::Done,
            ]))
        } else {
            Box::pin(stream::iter(vec![XaiEvent::Done]))
        }
    }
}

/// End-to-end: a tool hitting `RuleDecision::Ask` makes xai emit an ACP
/// `request_permission` over NATS; the IDE stand-in replies "reject"; the runner
/// gates (denies) the tool. Proves the runner-side half of the permission relay.
#[tokio::test]
async fn prompt_ask_emits_request_permission_and_gates_on_reply() {
    use acp_nats::acp_prefix::AcpPrefix;
    use agent_client_protocol::{
        RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome, SessionUpdate,
    };
    use std::time::Duration;

    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("start NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");

    // IDE stand-in: capture the permission request, reply "reject".
    let captured: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let captured_c = captured.clone();
    let responder = nats.clone();
    tokio::spawn(async move {
        let mut sub = responder
            .subscribe("acp.session.*.client.session.request_permission".to_string())
            .await
            .unwrap();
        while let Some(msg) = sub.next().await {
            *captured_c.lock().unwrap() = Some(String::from_utf8_lossy(&msg.payload).to_string());
            if let Some(reply) = msg.reply {
                let resp = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new("reject"),
                ));
                responder
                    .publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .ok();
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let notes = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Wire the production permission path: check_tool_permission → perm_tx →
    // handle_permission_request_nats → NATS, exactly as main.rs does. (The legacy
    // `with_permissions`/`ask_permission` path is no longer used by `prompt`.)
    let (perm_tx, mut perm_rx) = tokio::sync::mpsc::channel::<trogon_runner_tools::PermissionReq>(32);
    let perm_store = trogon_runner_tools::AllowedToolsSessionStore::new();
    let agent = XaiAgent::with_deps(
        RecordingNotifier { notes: notes.clone() },
        "grok-4",
        "dummy-key",
        ReadFileToolThenDone {
            calls: std::sync::atomic::AtomicUsize::new(0),
        },
    )
    .with_permission_gate(perm_tx, perm_store.clone());

    let nats_bridge = nats.clone();
    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                while let Some(req) = perm_rx.recv().await {
                    trogon_runner_tools::handle_permission_request_nats(
                        req,
                        nats_bridge.clone(),
                        AcpPrefix::new("acp").unwrap(),
                        &perm_store,
                    )
                    .await;
                }
            });
            let sid = agent
                .new_session(NewSessionRequest::new(tmp.path().to_path_buf()))
                .await
                .unwrap()
                .session_id;
            agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("read the file")]))
                .await
                .unwrap();
        })
        .await;

    // 1. xai emitted request_permission for the gated tool.
    let payload = captured
        .lock()
        .unwrap()
        .clone()
        .expect("xai must emit request_permission when a tool hits Ask");
    assert!(
        payload.contains("read_file"),
        "permission request must carry the tool name; got: {payload}"
    );

    // 2. The IDE's reject decision gated the tool (denied, never executed).
    let notes = notes.lock().unwrap();
    let denied = notes.iter().any(|n| match &n.update {
        SessionUpdate::ToolCallUpdate(tcu) => tcu
            .fields
            .raw_output
            .as_ref()
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase().contains("permission denied"))
            .unwrap_or(false),
        _ => false,
    });
    assert!(
        denied,
        "tool must be denied after the IDE rejected the permission request"
    );
}
