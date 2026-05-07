//! True end-to-end integration test: real NATS (testcontainers) + real OpenRouter API.
//!
//! Exercises the full stack in one shot:
//!   NATS request → AgentSideNatsConnection → OpenRouterAgent → OpenRouterClient → OpenRouter API
//!                                                                                ↓
//!   NATS notification ← NatsSessionNotifier ←────────────────── SSE text chunks
//!
//! Requires Docker (NATS container) and a funded OpenRouter account.
//!
//! Run with:
//!   OPENROUTER_TEST_KEY=sk-or-... cargo test -p trogon-openrouter-runner --test live_e2e -- --ignored

use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{AuthenticateRequest, ContentBlock, NewSessionRequest, PromptRequest};
use futures_util::StreamExt as _;
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use trogon_openrouter_runner::{NatsSessionNotifier, OpenRouterAgent, OpenRouterClient};

fn test_key() -> String {
    std::env::var("OPENROUTER_TEST_KEY")
        .expect("OPENROUTER_TEST_KEY must be set — see file header for usage")
}

async fn start_nats() -> (impl Drop, async_nats::Client) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (container, nats)
}

async fn start_agent_with_real_openrouter(nats: async_nats::Client, api_key: String, model: &str) {
    let model = model.to_string();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
        let http_client = OpenRouterClient::new();
        let agent = OpenRouterAgent::with_deps(notifier, &model, &api_key, http_client);
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    // Give agent time to subscribe.
    tokio::time::sleep(Duration::from_millis(400)).await;
}

async fn nats_req(nats: &async_nats::Client, subject: impl Into<String>, payload: Vec<u8>) -> Value {
    let subject = subject.into();
    let msg = tokio::time::timeout(
        Duration::from_secs(30),
        nats.request(subject.clone(), payload.into()),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out on {subject}"))
    .unwrap_or_else(|e| panic!("NATS request failed on {subject}: {e}"));
    serde_json::from_slice(&msg.payload).expect("valid JSON response")
}

async fn collect_text_chunks(sub: &mut async_nats::Subscriber) -> String {
    let mut chunks = Vec::new();
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(500), sub.next()).await
    {
        let v: Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
        if v["update"]["sessionUpdate"].as_str() == Some("agent_message_chunk") {
            if let Some(text) = v["update"]["content"]["text"].as_str() {
                chunks.push(text.to_string());
            }
        }
    }
    chunks.join("")
}

async fn do_initialize(nats: &async_nats::Client) {
    let payload = serde_json::to_vec(&serde_json::json!({"protocolVersion": 0})).unwrap();
    let resp = nats_req(nats, "acp.agent.initialize", payload).await;
    assert!(resp["protocolVersion"].is_number(), "missing protocolVersion: {resp}");
}

async fn do_authenticate_user_key(nats: &async_nats::Client, key: &str) {
    let mut meta = serde_json::Map::new();
    meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!(key));
    let req = AuthenticateRequest::new("openrouter-api-key").meta(meta);
    let payload = serde_json::to_vec(&req).unwrap();
    let resp = nats_req(nats, "acp.agent.authenticate", payload).await;
    assert!(resp.get("error").is_none(), "authenticate must not error: {resp}");
}

async fn do_authenticate_agent_key(nats: &async_nats::Client) {
    let req = AuthenticateRequest::new("agent");
    let payload = serde_json::to_vec(&req).unwrap();
    let resp = nats_req(nats, "acp.agent.authenticate", payload).await;
    assert!(resp.get("error").is_none(), "agent-key authenticate must not error: {resp}");
}

async fn do_new_session(nats: &async_nats::Client) -> String {
    let payload = serde_json::to_vec(&NewSessionRequest::new("/tmp")).unwrap();
    let resp = nats_req(nats, "acp.agent.session.new", payload).await;
    let session_id = resp["sessionId"]
        .as_str()
        .expect("sessionId must be a string")
        .to_string();
    assert!(!session_id.is_empty(), "sessionId must be non-empty");
    session_id
}

async fn do_prompt(nats: &async_nats::Client, session_id: &str, text: &str) -> Value {
    let session_id = session_id.to_string();
    let text = text.to_string();
    let payload = serde_json::to_vec(&PromptRequest::new(
        session_id.clone(),
        vec![ContentBlock::from(text)],
    ))
    .unwrap();
    let subject = format!("acp.session.{session_id}.agent.prompt");
    let resp = nats_req(nats, subject, payload).await;
    assert!(resp.get("error").is_none(), "prompt must not error: {resp}");
    resp
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Full stack smoke test: NATS request/reply through the agent to OpenRouter and back.
///
/// Flow:
///   1. initialize  — verify protocol version and capabilities
///   2. authenticate — store API key as pending (consumed by next session.new)
///   3. session/new  — obtain a session_id (pending key is bound to this session)
///   4. subscribe to session notifications
///   5. session/prompt — send a short prompt, verify real text + endTurn
///   6. verify at least one text chunk arrived via NATS notification
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn full_stack_nats_to_openrouter_and_back() {
    let key = test_key();
    let model = std::env::var("OPENROUTER_TEST_MODEL")
        .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());

    let (_container, nats) = start_nats().await;
    start_agent_with_real_openrouter(nats.clone(), key.clone(), &model).await;

    do_initialize(&nats).await;
    do_authenticate_user_key(&nats, &key).await;
    let session_id = do_new_session(&nats).await;
    eprintln!("session_id={session_id}");

    let notif_subject = format!("acp.session.{session_id}.client.session.update");
    let mut notif_sub = nats.subscribe(notif_subject).await.expect("subscribe");

    let resp = do_prompt(&nats, &session_id, "Reply with exactly one word: hello").await;
    assert_eq!(resp["stopReason"].as_str(), Some("end_turn"), "expected end_turn: {resp}");

    let full_text = collect_text_chunks(&mut notif_sub).await;
    eprintln!("full_text={full_text:?}");
    assert!(!full_text.is_empty(), "expected at least one text chunk via NATS notification");
}

/// Multi-turn conversation: verify conversation history is sent to OpenRouter.
///
/// Sends two prompts on the same session. The second prompt asks the model to
/// recall something from the first exchange, which only works if the accumulated
/// history array is correctly forwarded to OpenRouter.
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn multi_turn_conversation_history_is_maintained() {
    let key = test_key();
    let model = std::env::var("OPENROUTER_TEST_MODEL")
        .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());

    let (_container, nats) = start_nats().await;
    start_agent_with_real_openrouter(nats.clone(), key.clone(), &model).await;

    do_initialize(&nats).await;
    do_authenticate_user_key(&nats, &key).await;
    let session_id = do_new_session(&nats).await;
    eprintln!("session_id={session_id}");

    // ── first turn: plant a secret word ───────────────────────────────────────
    let notif_subject = format!("acp.session.{session_id}.client.session.update");
    let mut notif_sub = nats.subscribe(notif_subject).await.expect("subscribe");

    let resp1 = do_prompt(&nats, &session_id, "The secret word is BANANA. Acknowledge with just: OK").await;
    assert_eq!(resp1["stopReason"].as_str(), Some("end_turn"), "{resp1}");
    let text1 = collect_text_chunks(&mut notif_sub).await;
    eprintln!("turn1={text1:?}");
    assert!(!text1.is_empty(), "first turn must return text");

    // ── second turn: verify model recalls the secret word ─────────────────────
    let resp2 = do_prompt(&nats, &session_id, "What is the secret word I gave you? Reply with only that word.").await;
    assert_eq!(resp2["stopReason"].as_str(), Some("end_turn"), "{resp2}");
    let text2 = collect_text_chunks(&mut notif_sub).await;
    eprintln!("turn2={text2:?}");
    assert!(
        text2.to_uppercase().contains("BANANA"),
        "second turn must reference the secret word from history — got: {text2:?}"
    );
}

/// Bad model name: verify the stack handles an unknown model gracefully.
///
/// OpenRouter returns an HTTP error for an unknown model ID. The agent must not
/// panic or hang — it should log the error and return end_turn with no text.
/// This test costs nothing (no tokens are consumed).
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn bad_model_name_returns_end_turn_gracefully() {
    let key = test_key();

    let (_container, nats) = start_nats().await;
    start_agent_with_real_openrouter(nats.clone(), key.clone(), "openai/nonexistent-xyz-99999").await;

    do_initialize(&nats).await;
    do_authenticate_user_key(&nats, &key).await;
    let session_id = do_new_session(&nats).await;
    eprintln!("session_id={session_id}");

    let notif_subject = format!("acp.session.{session_id}.client.session.update");
    let mut notif_sub = nats.subscribe(notif_subject).await.expect("subscribe");

    let resp = do_prompt(&nats, &session_id, "hello").await;
    eprintln!("bad-model prompt response: {resp}");
    assert_eq!(
        resp["stopReason"].as_str(),
        Some("end_turn"),
        "bad model must still return end_turn (not hang or crash): {resp}"
    );

    let full_text = collect_text_chunks(&mut notif_sub).await;
    eprintln!("bad-model full_text={full_text:?}");
    assert!(
        full_text.is_empty(),
        "bad model must produce no text chunks — got: {full_text:?}"
    );
}

/// Agent-key auth: authenticate using the server-configured API key.
///
/// The agent is started with the real key as global_api_key. Authenticating with
/// method "agent" skips the pending-key flow — new_session() picks up global_api_key
/// directly. Verifies that the "Use server key" auth path works end-to-end.
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn agent_key_auth_uses_server_configured_key() {
    let key = test_key();
    let model = std::env::var("OPENROUTER_TEST_MODEL")
        .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());

    let (_container, nats) = start_nats().await;
    start_agent_with_real_openrouter(nats.clone(), key.clone(), &model).await;

    do_initialize(&nats).await;
    // Use "agent" method — no API key in meta, relies on global_api_key set at startup.
    do_authenticate_agent_key(&nats).await;
    let session_id = do_new_session(&nats).await;
    eprintln!("session_id={session_id}");

    let notif_subject = format!("acp.session.{session_id}.client.session.update");
    let mut notif_sub = nats.subscribe(notif_subject).await.expect("subscribe");

    let resp = do_prompt(&nats, &session_id, "Reply with exactly one word: yes").await;
    assert_eq!(resp["stopReason"].as_str(), Some("end_turn"), "{resp}");

    let full_text = collect_text_chunks(&mut notif_sub).await;
    eprintln!("agent-key full_text={full_text:?}");
    assert!(!full_text.is_empty(), "agent-key session must produce text chunks via NATS");
}
