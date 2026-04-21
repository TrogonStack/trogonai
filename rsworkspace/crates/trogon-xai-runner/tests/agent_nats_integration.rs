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
    PromptRequest, SessionNotification,
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
        _tools: &[String],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        Box::pin(stream::empty())
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
        _tools: &[String],
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
    let store = NatsSessionStore::open(&js).await.expect("store");
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
    let store = NatsSessionStore::open(&js).await.expect("store");
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
    let store = NatsSessionStore::open(&js).await.expect("store");
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
    let store = NatsSessionStore::open(&js).await.expect("store");
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
    let store = NatsSessionStore::open(&js).await.expect("store");
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
    let store = NatsSessionStore::open(&js).await.expect("store");
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
    let store = NatsSessionStore::open(&js).await.expect("store");

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
    let store = NatsSessionStore::open(&js).await.expect("store");

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

// ── prompt with assistant response persists both messages ─────────────────────

#[tokio::test]
async fn prompt_with_assistant_response_persists_both_messages_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js).await.expect("store");
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
