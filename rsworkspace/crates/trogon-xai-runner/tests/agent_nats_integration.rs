//! Integration tests verifying that `XaiAgent` persists sessions to real NATS JetStream.
//!
//! These tests spin up a fresh NATS container and verify that `new_session`,
//! `fork_session`, `close_session`, `prompt`, and the `with_loaders` /
//! `TENANT_ID` wiring all write the expected data to the SESSIONS KV bucket.
//! The xAI HTTP client and session notifier are no-op stubs — no real network needed.

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::{
    Agent as _, CloseSessionRequest, ContentBlock, ForkSessionRequest, NewSessionRequest,
    PromptRequest, SessionNotification, SetSessionModelRequest,
};
use async_nats::jetstream;
use async_trait::async_trait;
use futures_util::stream::{self, LocalBoxStream};
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_xai_runner::{
    AgentLoader, InputItem, SessionNotifier, SkillLoader, XaiAgent,
    XaiEvent, XaiHttpClient,
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
        Box::pin(stream::iter(vec![
            XaiEvent::TextDelta {
                text: "Hello back!".to_string(),
            },
        ]))
    }
}

fn make_agent(store: NatsSessionStore) -> XaiAgent<NoOpHttpClient, NoOpNotifier> {
    XaiAgent::with_deps(NoOpNotifier, "grok-4", "", NoOpHttpClient)
        .with_session_store(Arc::new(store))
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
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .unwrap();
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
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .unwrap();
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
                .fork_session(ForkSessionRequest::new(
                    source_id.clone(),
                    PathBuf::from("/tmp"),
                ))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id.to_string();

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");

            let source_key = format!("default.{}", source_id);
            assert!(
                kv.get(&source_key).await.unwrap().is_some(),
                "source entry must exist"
            );

            let fork_key = format!("default.{fork_id}");
            let bytes = kv
                .get(&fork_key)
                .await
                .unwrap()
                .expect("fork entry must exist");
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
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", NoOpHttpClient)
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
    kv.put(key, bytes::Bytes::from(json.to_string()))
        .await
        .expect("KV put");
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
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "", NoOpHttpClient)
        .with_session_store(Arc::new(store));
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
                .fork_session(ForkSessionRequest::new(
                    src_id.clone(),
                    PathBuf::from("/fork"),
                ))
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
                .fork_session(
                    ForkSessionRequest::new(src_id.clone(), PathBuf::from("/fork")).meta(meta),
                )
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
                .set_session_model(SetSessionModelRequest::new(
                    session_id.clone(),
                    "grok-3-mini",
                ))
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
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyHttpClient)
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
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyHttpClient)
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
    let agent = XaiAgent::with_deps(NoOpNotifier, "grok-4", "dummy-key", ReplyHttpClient)
        .with_session_store(Arc::new(store));

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

            assert!(
                name.ends_with('…'),
                "truncated name must end with '…'; got: {name:?}"
            );
            // 60 ASCII chars + the 3-byte UTF-8 ellipsis = 63 bytes, 61 chars.
            assert_eq!(
                name.chars().count(),
                61,
                "truncated name must be 60 chars + '…'; got: {name:?}"
            );
        })
        .await;
}
