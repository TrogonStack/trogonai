#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests verifying that `OpenRouterAgent` persists sessions to real NATS JetStream.
//!
//! The OpenRouter HTTP client and session notifier are no-op stubs — no real network needed.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test agent_nats_integration
#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, Mutex};

use std::path::PathBuf;

use acp_nats::AgentHandler;
use agent_client_protocol::schema::v1::{
    BlobResourceContents, CancelNotification, CloseSessionRequest, ContentBlock, EmbeddedResource,
    EmbeddedResourceResource, ForkSessionRequest, NewSessionRequest, PromptRequest, ResourceLink, SessionId,
    SessionNotification, TextResourceContents,
};
use async_nats::jetstream;
use async_trait::async_trait;
use futures_util::stream::{self, BoxStream, StreamExt as _};
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_openrouter_runner::{
    AgentLoader, AssembledToolCall, Message, OpenRouterAgent, OpenRouterEvent, OpenRouterHttpClient, SessionNotifier,
    SkillLoader, ToolDef, session_store::NatsSessionStore,
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

#[async_trait]
impl SessionNotifier for NoOpNotifier {
    async fn notify(&self, _: SessionNotification) {}
}

struct NoOpHttpClient;

#[async_trait]
impl OpenRouterHttpClient for NoOpHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::empty())
    }
}

/// Returns a fixed assistant reply ("Hello back!") then ends.
struct ReplyHttpClient;

/// First call returns a bash tool call; second call returns final text.
struct ToolRoundHttpClient {
    call_count: Arc<Mutex<u32>>,
}

impl ToolRoundHttpClient {
    fn new() -> Self {
        Self {
            call_count: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl OpenRouterHttpClient for ToolRoundHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        let mut n = self.call_count.lock().unwrap();
        *n += 1;
        let call = *n;
        drop(n);

        if call == 1 {
            Box::pin(stream::iter(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_1".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]))
        } else {
            Box::pin(stream::iter(vec![OpenRouterEvent::TextDelta {
                text: "answer".to_string(),
            }]))
        }
    }
}

#[async_trait]
impl OpenRouterHttpClient for ReplyHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::iter(vec![OpenRouterEvent::TextDelta {
            text: "Hello back!".to_string(),
        }]))
    }
}

/// Returns text reply + usage event (10 prompt / 5 completion tokens).
struct UsageHttpClient;

#[async_trait]
impl OpenRouterHttpClient for UsageHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::iter(vec![
            OpenRouterEvent::TextDelta {
                text: "reply".to_string(),
            },
            OpenRouterEvent::Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        ]))
    }
}

/// First call: emits partial text then a tool call. Second call: emits final text.
struct ThinkThenToolHttpClient {
    call_count: Arc<Mutex<u32>>,
}

impl ThinkThenToolHttpClient {
    fn new() -> Self {
        Self {
            call_count: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl OpenRouterHttpClient for ThinkThenToolHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        let mut n = self.call_count.lock().unwrap();
        *n += 1;
        let call = *n;
        drop(n);

        if call == 1 {
            Box::pin(stream::iter(vec![
                OpenRouterEvent::TextDelta {
                    text: "thinking...".to_string(),
                },
                OpenRouterEvent::ToolCallsReady {
                    calls: vec![AssembledToolCall {
                        id: "call_t".to_string(),
                        name: "list_directory".to_string(),
                        arguments: r#"{"path":"."}"#.to_string(),
                    }],
                },
            ]))
        } else {
            Box::pin(stream::iter(vec![OpenRouterEvent::TextDelta {
                text: "done".to_string(),
            }]))
        }
    }
}

/// Emits one partial TextDelta then an Error — simulates a mid-stream failure.
struct PartialThenErrorHttpClient;

#[async_trait]
impl OpenRouterHttpClient for PartialThenErrorHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::iter(vec![
            OpenRouterEvent::TextDelta {
                text: "partial".to_string(),
            },
            OpenRouterEvent::Error {
                message: "upstream boom".to_string(),
            },
        ]))
    }
}

fn make_agent(store: NatsSessionStore) -> OpenRouterAgent<NoOpHttpClient, NoOpNotifier> {
    OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "", NoOpHttpClient).with_session_store(Arc::new(store))
}

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

            assert_eq!(v["model"], "test-model");
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

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let key = format!("default.{session_id}");
            assert!(
                kv.get(&key).await.unwrap().is_some(),
                "snapshot must remain in NATS after close"
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_persists_user_message_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
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

            let messages = v["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 1, "one user message expected");
            assert_eq!(messages[0]["role"], "user");
            assert_eq!(messages[0]["content"][0]["text"], "Hello from integration test");
        })
        .await;
}

#[tokio::test]
async fn new_session_with_loaders_writes_console_model_to_sessions() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-x",
        r#"{"skill_ids":[],"model":{"id":"openai/gpt-4o"}}"#,
    )
    .await;

    let agent_loader = AgentLoader::open(&js).await.expect("AgentLoader");
    let skill_loader = SkillLoader::open(&js).await.expect("SkillLoader");
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "", NoOpHttpClient)
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

            assert_eq!(v["model"], "openai/gpt-4o");
        })
        .await;
}

#[tokio::test]
async fn tenant_id_env_var_sets_kv_key_prefix() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    unsafe {
        std::env::set_var("TENANT_ID", "acme-corp");
    }
    let agent =
        OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "", NoOpHttpClient).with_session_store(Arc::new(store));
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

            let custom_key = format!("acme-corp.{session_id}");
            let bytes = kv
                .get(&custom_key)
                .await
                .unwrap()
                .expect("entry must exist under custom tenant prefix");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["tenant_id"], "acme-corp");

            let default_key = format!("default.{session_id}");
            assert!(
                kv.get(&default_key).await.unwrap().is_none(),
                "entry must not exist under 'default' prefix"
            );
        })
        .await;
}

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

#[tokio::test]
async fn prompt_with_assistant_response_persists_both_messages_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", ReplyHttpClient)
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

#[tokio::test]
async fn prompt_with_tool_calls_persists_only_text_messages_to_nats() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", ToolRoundHttpClient::new())
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
                    vec![ContentBlock::from("list files".to_string())],
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

            let messages = v["messages"].as_array().unwrap();
            assert_eq!(
                messages.len(),
                2,
                "snapshot must have exactly user + assistant text (tool_calls/tool_result filtered out): {messages:?}"
            );
            assert_eq!(messages[0]["role"], "user", "first message must be user");
            assert_eq!(messages[1]["role"], "assistant", "second message must be assistant");
            assert!(
                messages[1].get("tool_calls").is_none() || messages[1]["tool_calls"].is_null(),
                "assistant snapshot message must not contain tool_calls: {}",
                messages[1]
            );
            for msg in messages.iter() {
                assert!(
                    msg.get("tool_call_id").is_none() || msg["tool_call_id"].is_null(),
                    "snapshot must not contain tool_result messages: {msg}"
                );
            }
        })
        .await;
}

#[tokio::test]
async fn session_name_set_from_first_prompt_message() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            // Before prompt the name defaults to "New Conversation".
            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv.get(&format!("default.{session_id}")).await.unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["name"], "New Conversation", "before prompt: default name expected");

            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("What is the capital of France?".to_string())],
                ))
                .await
                .unwrap();

            let bytes = kv.get(&format!("default.{session_id}")).await.unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                v["name"], "What is the capital of France?",
                "after first prompt: KV name must be derived from user message"
            );
        })
        .await;
}

#[tokio::test]
async fn fork_with_branch_at_index_stores_truncated_messages_to_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", ReplyHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let new_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let src_id = new_resp.session_id.clone();

            // Source gets a user + assistant message.
            agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::from("hi".to_string())],
                ))
                .await
                .unwrap();

            // Fork at index 0 — no messages should be carried over.
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
            let messages = v["messages"].as_array().unwrap();
            assert_eq!(
                messages.len(),
                0,
                "fork with branchAtIndex=0 must store 0 messages in KV snapshot: {messages:?}"
            );
        })
        .await;
}

#[tokio::test]
async fn resume_detection_skips_duplicate_user_message() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            // First prompt — adds user message to history.
            agent
                .prompt(PromptRequest::new(
                    resp.session_id.clone(),
                    vec![ContentBlock::from("hello".to_string())],
                ))
                .await
                .unwrap();

            // Second prompt with identical content simulates reconnect after crash.
            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("hello".to_string())],
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
            let user_msgs: Vec<_> = messages.iter().filter(|m| m["role"] == "user").collect();
            assert_eq!(
                user_msgs.len(),
                1,
                "duplicate user message on resume must be skipped; KV messages: {messages:?}"
            );
            assert_eq!(user_msgs[0]["content"][0]["text"], "hello");
        })
        .await;
}

#[tokio::test]
async fn system_prompt_not_persisted_to_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
        .with_system_prompt("You are a helpful assistant.")
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
                    vec![ContentBlock::from("hi".to_string())],
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
            let system_msgs: Vec<_> = messages.iter().filter(|m| m["role"] == "system").collect();
            assert_eq!(
                system_msgs.len(),
                0,
                "system prompt must not be persisted to KV snapshot: {messages:?}"
            );
        })
        .await;
}

#[tokio::test]
async fn resource_link_content_block_stored_formatted() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
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
                    vec![ContentBlock::ResourceLink(ResourceLink::new(
                        "readme",
                        "file:///readme.md",
                    ))],
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
            assert_eq!(messages.len(), 1, "one user message expected");
            let content = messages[0]["content"][0]["text"].as_str().unwrap_or("");
            assert_eq!(
                content, "[Resource: readme | file:///readme.md]",
                "ResourceLink must be stored as formatted string in KV"
            );
        })
        .await;
}

#[tokio::test]
async fn fork_inherits_model_from_source_in_kv() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-x",
        r#"{"skill_ids":[],"model":{"id":"openai/gpt-4o"}}"#,
    )
    .await;

    let agent_loader = AgentLoader::open(&js).await.expect("AgentLoader");
    let skill_loader = SkillLoader::open(&js).await.expect("SkillLoader");
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "", NoOpHttpClient)
        .with_loaders("agent-x", Arc::new(agent_loader), Arc::new(skill_loader))
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let src_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
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
                v["model"], "openai/gpt-4o",
                "fork must inherit source model in KV snapshot; got: {}",
                v["model"]
            );
        })
        .await;
}

#[tokio::test]
async fn embedded_text_resource_block_stored_verbatim() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
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
                    vec![ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                            "fn main() {}",
                            "file:///src/main.rs",
                        )),
                    ))],
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
            assert_eq!(messages.len(), 1, "one user message expected");
            let content = messages[0]["content"][0]["text"].as_str().unwrap_or("");
            assert_eq!(
                content, "fn main() {}",
                "TextResourceContents text must be stored verbatim in KV"
            );
        })
        .await;
}

#[tokio::test]
async fn multiple_content_blocks_joined_with_newline_in_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
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
                    vec![
                        ContentBlock::from("first block".to_string()),
                        ContentBlock::from("second block".to_string()),
                    ],
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
            assert_eq!(messages.len(), 1, "one user message expected");
            let content = messages[0]["content"][0]["text"].as_str().unwrap_or("");
            assert_eq!(
                content, "first block\nsecond block",
                "multiple ContentBlock::Text must be joined with newline in KV"
            );
        })
        .await;
}

#[tokio::test]
async fn usage_tokens_stored_in_kv_assistant_message() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", UsageHttpClient)
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
                    vec![ContentBlock::from("hello".to_string())],
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
            let asst = messages
                .iter()
                .find(|m| m["role"] == "assistant")
                .expect("assistant message must exist in KV");
            assert_eq!(
                asst["usage"]["input_tokens"], 10,
                "assistant message must store prompt_tokens as usage.input_tokens"
            );
            assert_eq!(
                asst["usage"]["output_tokens"], 5,
                "assistant message must store completion_tokens as usage.output_tokens"
            );
        })
        .await;
}

// env var tests share a mutex to avoid parallel mutation races
static ENV_MUTEX: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

#[tokio::test]
async fn history_trimmed_to_max_history_in_kv() {
    let _lock = ENV_MUTEX.get_or_init(|| Mutex::new(())).lock().unwrap();

    unsafe {
        std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "2");
    }
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", ReplyHttpClient)
        .with_session_store(Arc::new(store));
    unsafe {
        std::env::remove_var("OPENROUTER_MAX_HISTORY_MESSAGES");
    }

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            // Send 3 prompts: each adds user + assistant = 2 messages.
            // With max_history=2, only the last turn (2 messages) survives.
            for i in 0..3u32 {
                agent
                    .prompt(PromptRequest::new(
                        resp.session_id.clone(),
                        vec![ContentBlock::from(format!("turn {i}"))],
                    ))
                    .await
                    .unwrap();
            }

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("entry must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let messages = v["messages"].as_array().unwrap();
            assert!(
                messages.len() <= 2,
                "history must be trimmed to max_history=2 in KV; got {} messages: {messages:?}",
                messages.len()
            );
        })
        .await;
}

#[tokio::test]
async fn new_session_with_loaders_stores_agent_id_in_kv() {
    let (js, _c) = make_js().await;

    kv_put(
        &js,
        "CONSOLE_AGENTS",
        "agent-x",
        r#"{"skill_ids":[],"model":{"id":"openai/gpt-4o"}}"#,
    )
    .await;

    let agent_loader = AgentLoader::open(&js).await.expect("AgentLoader");
    let skill_loader = SkillLoader::open(&js).await.expect("SkillLoader");
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "", NoOpHttpClient)
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
            assert_eq!(
                v["agent_id"], "agent-x",
                "KV snapshot must include agent_id when agent uses loaders"
            );
        })
        .await;
}

#[tokio::test]
async fn partial_text_before_error_persisted_to_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", PartialThenErrorHttpClient)
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
                    vec![ContentBlock::from("q".to_string())],
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
            assert_eq!(
                messages.len(),
                2,
                "partial text before error must produce user + assistant messages in KV: {messages:?}"
            );
            assert_eq!(messages[0]["role"].as_str().unwrap_or(""), "user");
            assert_eq!(messages[1]["role"].as_str().unwrap_or(""), "assistant");
            assert_eq!(
                messages[1]["content"][0]["text"].as_str().unwrap_or(""),
                "partial",
                "assistant message must contain the partial text collected before the error"
            );
        })
        .await;
}

#[tokio::test]
async fn fork_with_branch_index_beyond_history_copies_full_history_in_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", ReplyHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let new_resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let src_id = new_resp.session_id.clone();

            // Prompt so source has [user, assistant] in history (2 messages).
            agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::from("hi".to_string())],
                ))
                .await
                .unwrap();

            // Fork with branchAtIndex=999 — beyond history length, so full copy.
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 999 }),
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
            let messages = v["messages"].as_array().unwrap();
            assert_eq!(
                messages.len(),
                2,
                "fork with branchAtIndex beyond history length must copy all messages: {messages:?}"
            );
            assert_eq!(messages[0]["role"].as_str().unwrap_or(""), "user");
            assert_eq!(messages[1]["role"].as_str().unwrap_or(""), "assistant");
        })
        .await;
}

#[tokio::test]
async fn blob_resource_content_block_stored_formatted_in_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            let blob = BlobResourceContents::new("base64data==", "file:///img.png").mime_type("image/png");
            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::BlobResourceContents(blob),
                    ))],
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
            assert_eq!(messages.len(), 1, "one user message expected");
            let content = messages[0]["content"][0]["text"].as_str().unwrap_or("");
            assert_eq!(
                content, "[Binary resource: file:///img.png (image/png)]",
                "BlobResourceContents must be stored as a binary placeholder in KV"
            );
        })
        .await;
}

#[tokio::test]
async fn empty_assistant_response_not_stored_in_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");

    // HTTP client returns nothing — no TextDelta, just empty stream.
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", NoOpHttpClient)
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
                    vec![ContentBlock::from("hello".to_string())],
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
            assert_eq!(
                messages.len(),
                1,
                "only the user message must be stored; no empty assistant message: {messages:?}"
            );
            assert_eq!(
                messages[0]["role"].as_str().unwrap_or(""),
                "user",
                "the single stored message must be the user message"
            );
        })
        .await;
}

#[tokio::test]
async fn partial_text_before_tool_calls_not_in_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", ThinkThenToolHttpClient::new())
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
                    vec![ContentBlock::from("q".to_string())],
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

            // Must have user + final assistant("done"), not a standalone "thinking..." message.
            assert_eq!(
                messages.len(),
                2,
                "KV must have exactly user + final assistant message: {messages:?}"
            );
            assert_eq!(messages[0]["role"].as_str().unwrap_or(""), "user");
            assert_eq!(messages[1]["role"].as_str().unwrap_or(""), "assistant");

            let assistant_text = messages[1]["content"][0]["text"].as_str().unwrap_or("");
            assert_eq!(
                assistant_text, "done",
                "assistant message must be the final text, not the pre-tool partial: {messages:?}"
            );
            let has_thinking = messages
                .iter()
                .any(|m| m["content"][0]["text"].as_str().unwrap_or("").contains("thinking"));
            assert!(
                !has_thinking,
                "partial text emitted before tool calls must not appear in KV: {messages:?}"
            );
        })
        .await;
}

// ── PR 15: _meta.systemPrompt stored in session state ────────────────────────

/// Verifies that _meta.systemPrompt set on new_session is stored in the
/// in-memory session and readable via ext session/get_state.
#[tokio::test]
async fn new_session_meta_system_prompt_stored_in_session_state() {
    use agent_client_protocol::schema::v1::ExtRequest;

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
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", UsageHttpClient)
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
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", UsageHttpClient)
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

// ── cache tokens in KV ────────────────────────────────────────────────────────

/// HTTP client that returns non-zero cache_read and cache_creation tokens.
struct CacheUsageHttpClient;

#[async_trait]
impl OpenRouterHttpClient for CacheUsageHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::iter(vec![
            OpenRouterEvent::TextDelta {
                text: "cached".to_string(),
            },
            OpenRouterEvent::Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                cache_read_tokens: 30,
                cache_creation_tokens: 15,
            },
        ]))
    }
}

/// After a prompt with non-zero cache_read_tokens and cache_creation_tokens,
/// both fields must be persisted to the SESSIONS KV bucket.
#[tokio::test]
async fn cache_tokens_persisted_to_sessions_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", CacheUsageHttpClient)
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
                v["total_cache_read_tokens"], 30,
                "total_cache_read_tokens must be 30; got: {v}"
            );
            assert_eq!(
                v["total_cache_creation_tokens"], 15,
                "total_cache_creation_tokens must be 15; got: {v}"
            );
        })
        .await;
}

/// Token totals must accumulate across multiple prompts in the same session.
/// UsageHttpClient returns 10 prompt + 5 completion per call, so three prompts
/// must yield total_input_tokens=30 and total_output_tokens=15.
#[tokio::test]
async fn token_totals_accumulate_across_three_prompts() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let agent = OpenRouterAgent::with_deps(NoOpNotifier, "test-model", "dummy-key", UsageHttpClient)
        .with_session_store(Arc::new(store));

    tokio::task::LocalSet::new()
        .run_until(async move {
            let resp = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/tmp")))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();

            for i in 0..3u32 {
                agent
                    .prompt(PromptRequest::new(
                        resp.session_id.clone(),
                        vec![ContentBlock::from(format!("prompt {i}"))],
                    ))
                    .await
                    .unwrap();
            }

            let kv = js.get_key_value("SESSIONS").await.expect("get KV");
            let bytes = kv
                .get(&format!("default.{session_id}"))
                .await
                .unwrap()
                .expect("snapshot must exist");
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

            assert_eq!(
                v["total_input_tokens"], 30,
                "total_input_tokens must accumulate to 30 across 3 prompts (10 each); got: {v}"
            );
            assert_eq!(
                v["total_output_tokens"], 15,
                "total_output_tokens must accumulate to 15 across 3 prompts (5 each); got: {v}"
            );
        })
        .await;
}

// ── cancel path persists tokens to KV ────────────────────────────────────────

/// HTTP client that emits Usage + text then blocks forever.
/// Signals `ready` when `chat_stream` is invoked so the test can send cancel
/// after Usage has been processed.
struct SlowCancelHttpClient {
    ready: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl OpenRouterHttpClient for SlowCancelHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        self.ready.notify_one();
        let initial = stream::iter(vec![
            OpenRouterEvent::TextDelta {
                text: "partial".to_string(),
            },
            OpenRouterEvent::Usage {
                prompt_tokens: 12,
                completion_tokens: 7,
                cache_read_tokens: 25,
                cache_creation_tokens: 10,
            },
        ]);
        Box::pin(initial.chain(stream::pending()))
    }
}

/// When a prompt is cancelled after the OpenRouter Usage event has been
/// received, the cancel path must persist the billed tokens to the SESSIONS KV
/// bucket.
#[tokio::test]
async fn cancel_prompt_token_totals_persisted_to_kv() {
    let (js, _c) = make_js().await;
    let store = NatsSessionStore::open(&js, 0).await.expect("store");
    let ready = Arc::new(tokio::sync::Notify::new());
    let agent = Arc::new(
        OpenRouterAgent::with_deps(
            NoOpNotifier,
            "test-model",
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

            // `chat_stream` was called → Usage event is in the stream buffer.
            // Cancel fires after Usage has been processed by the streaming loop.
            ready.notified().await;
            agent.cancel(CancelNotification::new(session_id.clone())).await.unwrap();

            let result = prompt_handle.await.unwrap();
            assert_eq!(
                result.stop_reason,
                agent_client_protocol::schema::v1::StopReason::Cancelled,
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
                v["total_input_tokens"], 12,
                "cancel path must persist input tokens; got: {v}"
            );
            assert_eq!(
                v["total_output_tokens"], 7,
                "cancel path must persist output tokens; got: {v}"
            );
            assert_eq!(
                v["total_cache_read_tokens"], 25,
                "cancel path must persist cache_read tokens; got: {v}"
            );
            assert_eq!(
                v["total_cache_creation_tokens"], 10,
                "cancel path must persist cache_creation tokens; got: {v}"
            );
        })
        .await;
}

// ── Context compaction end-to-end (Gap 2: reuse own tail) ────────────────────────

/// Drives a prompt whose history exceeds 85 % of the model's window, with a
/// stand-in compactor responder. Verifies openrouter compacts PRE-request via
/// NATS AND applies Gap 2 correctly: it reuses its OWN original tail message
/// (by `kept_count`), NOT the responder's kept message — so tool_calls/tool
/// pairing is never reconstructed.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn prompt_compacts_pre_request_and_reuses_own_tail() {
    use std::time::Duration;
    use trogon_openrouter_runner::{MockOpenRouterHttpClient, MockSessionNotifier};

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

    // Stand-in compactor: returns summary + ack + a DIFFERENT placeholder for the
    // kept message, plus kept_count=1. If openrouter (wrongly) used the responder's
    // kept message, the tail would be "RESPONDER-PLACEHOLDER"; Gap 2 means it must
    // instead reuse its OWN original tail ("RECENT-ORIGINAL").
    let responder = nats.clone();
    tokio::spawn(async move {
        let mut sub = responder.subscribe("trogon.compactor.compact").await.unwrap();
        while let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                let body = serde_json::json!({
                    "messages": [
                        {"role": "user", "content": [{"type": "text", "text": "<context-summary>\nsummary\n</context-summary>"}]},
                        {"role": "assistant", "content": [{"type": "text", "text": "ack"}]},
                        {"role": "user", "content": [{"type": "text", "text": "RESPONDER-PLACEHOLDER"}]}
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

    // gpt-4 → 8192-token window; 85 % ≈ 6963 tokens ≈ 28 KB. Mock LLM returns a
    // reply for the turn (test helpers live on the Mock-typed agent).
    let http = MockOpenRouterHttpClient::new();
    http.push_response(vec![OpenRouterEvent::TextDelta {
        text: "Hello back!".to_string(),
    }]);
    let agent = OpenRouterAgent::with_deps(MockSessionNotifier::new(), "gpt-4", "test-key", http)
        .with_session_store(Arc::new(store))
        .with_compactor(nats.clone());

    // 12 messages × ~2.5 KB ≈ 30 KB ≈ 7500 tokens > threshold. Last message is the
    // recognizable original tail that Gap 2 must reuse verbatim.
    let pad = "y".repeat(2_500);
    let mut history = Vec::new();
    for i in 0..11 {
        history.push(Message::user(format!("old {i} {pad}")));
    }
    history.push(Message::user(format!("RECENT-ORIGINAL {pad}")));
    agent.test_insert_session_with_history("s-or-compact", history).await;

    tokio::task::LocalSet::new()
        .run_until(async move {
            agent
                .prompt(PromptRequest::new(
                    SessionId::from("s-or-compact"),
                    vec![ContentBlock::from("hi".to_string())],
                ))
                .await
                .unwrap();

            let hist = agent.test_session_history("s-or-compact").await;
            // First message is the summary checkpoint.
            assert!(
                hist[0].content.contains("context-summary"),
                "first message must be the summary; got: {:?}",
                hist[0].content
            );
            // Gap 2: the kept tail is the runner's OWN original message…
            assert!(
                hist.iter().any(|m| m.content.contains("RECENT-ORIGINAL")),
                "must reuse the runner's own original tail message"
            );
            // …NOT the responder's placeholder.
            assert!(
                !hist.iter().any(|m| m.content.contains("RESPONDER-PLACEHOLDER")),
                "must NOT use the compactor's kept message (Gap 2: no reverse conversion)"
            );
        })
        .await;
}

// ── Permission emit e2e: openrouter emits request_permission on Ask and gates on the reply ──

/// Notifier that records every notification so the test can assert the deny update.
struct OrRecordingNotifier {
    notes: Arc<Mutex<Vec<SessionNotification>>>,
}

#[async_trait]
impl SessionNotifier for OrRecordingNotifier {
    async fn notify(&self, n: SessionNotification) {
        self.notes.lock().unwrap().push(n);
    }
}

/// First turn asks to call the client-side `read_file` tool (no permission rule →
/// `Ask`); ends on the continuation turn.
struct AskReadFileThenDone {
    calls: Arc<Mutex<u32>>,
}

#[async_trait]
impl OpenRouterHttpClient for AskReadFileThenDone {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> BoxStream<'static, OpenRouterEvent> {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        let call = *n;
        drop(n);
        if call == 1 {
            Box::pin(stream::iter(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "c1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"secret.txt"}"#.to_string(),
                }],
            }]))
        } else {
            Box::pin(stream::iter(vec![OpenRouterEvent::TextDelta {
                text: "done".to_string(),
            }]))
        }
    }
}

/// End-to-end: a tool hitting `RuleDecision::Ask` makes openrouter emit an ACP
/// `request_permission` over NATS; the IDE stand-in replies "reject"; the runner
/// gates (denies) the tool. Proves the runner-side half of the permission relay.
#[tokio::test]
async fn prompt_ask_emits_request_permission_and_gates_on_reply() {
    use acp_nats::acp_prefix::AcpPrefix;
    use agent_client_protocol::schema::v1::{
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
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
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

    let notes = Arc::new(Mutex::new(Vec::new()));
    let agent = OpenRouterAgent::with_deps(
        OrRecordingNotifier { notes: notes.clone() },
        "openai/gpt-4o",
        "dummy-key",
        AskReadFileThenDone {
            calls: Arc::new(Mutex::new(0)),
        },
    )
    .with_permissions(nats.clone(), AcpPrefix::new("acp").unwrap());

    tokio::task::LocalSet::new()
        .run_until(async move {
            let sid = agent
                .new_session(NewSessionRequest::new(PathBuf::from("/")))
                .await
                .unwrap()
                .session_id;
            agent
                .prompt(PromptRequest::new(sid, vec![ContentBlock::from("read the file")]))
                .await
                .unwrap();
        })
        .await;

    // 1. openrouter emitted request_permission for the gated tool.
    let payload = captured
        .lock()
        .unwrap()
        .clone()
        .expect("openrouter must emit request_permission when a tool hits Ask");
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
            .map(|s| s.contains("permission denied"))
            .unwrap_or(false),
        _ => false,
    });
    assert!(
        denied,
        "tool must be denied after the IDE rejected the permission request"
    );
}
