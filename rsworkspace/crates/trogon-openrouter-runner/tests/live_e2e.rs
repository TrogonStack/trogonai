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

    // ── 1. initialize ─────────────────────────────────────────────────────────
    let init_payload = serde_json::to_vec(&serde_json::json!({"protocolVersion": 0})).unwrap();
    let init_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.initialize", init_payload.into()),
    )
    .await
    .expect("timed out on initialize")
    .expect("NATS request failed");

    let init_resp: Value = serde_json::from_slice(&init_msg.payload).unwrap();
    eprintln!("initialize response: {init_resp}");
    assert!(init_resp["protocolVersion"].is_number(), "missing protocolVersion: {init_resp}");
    assert!(init_resp["agentCapabilities"].is_object(), "missing agentCapabilities: {init_resp}");

    // ── 2. authenticate — stores key as pending; consumed by next session.new ──
    // authenticate() sets pending_api_key; new_session() binds it to the session.
    let mut auth_meta = serde_json::Map::new();
    auth_meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!(key));
    let auth_req = AuthenticateRequest::new("openrouter-api-key").meta(auth_meta);
    let auth_payload = serde_json::to_vec(&auth_req).unwrap();
    let auth_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.authenticate", auth_payload.into()),
    )
    .await
    .expect("timed out on authenticate")
    .expect("NATS request failed");

    let auth_resp: Value = serde_json::from_slice(&auth_msg.payload).unwrap();
    eprintln!("authenticate response: {auth_resp}");
    assert!(
        auth_resp.get("error").is_none(),
        "authenticate must not return an error: {auth_resp}"
    );

    // ── 3. session/new — consumes the pending API key ─────────────────────────
    let new_session_payload = serde_json::to_vec(&NewSessionRequest::new("/tmp")).unwrap();
    let session_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", new_session_payload.into()),
    )
    .await
    .expect("timed out on session.new")
    .expect("NATS request failed");

    let session_resp: Value = serde_json::from_slice(&session_msg.payload).unwrap();
    let session_id = session_resp["sessionId"]
        .as_str()
        .expect("sessionId must be a string")
        .to_string();
    eprintln!("session_id={session_id}");
    assert!(!session_id.is_empty(), "sessionId must be non-empty");

    // ── 4. subscribe to session notifications BEFORE sending prompt ───────────
    let notif_subject = format!("acp.session.{session_id}.client.session.update");
    let mut notif_sub = nats
        .subscribe(notif_subject.clone())
        .await
        .expect("subscribe to notifications");

    // ── 5. prompt ─────────────────────────────────────────────────────────────
    let prompt_payload = serde_json::to_vec(&PromptRequest::new(
        session_id.clone(),
        vec![ContentBlock::from("Reply with exactly one word: hello".to_string())],
    ))
    .unwrap();
    let prompt_subject = format!("acp.session.{session_id}.agent.prompt");
    let prompt_msg = tokio::time::timeout(
        Duration::from_secs(30),
        nats.request(prompt_subject, prompt_payload.into()),
    )
    .await
    .expect("timed out waiting for prompt response")
    .expect("NATS prompt request failed");

    let prompt_resp: Value = serde_json::from_slice(&prompt_msg.payload).unwrap();
    eprintln!("prompt response: {prompt_resp}");
    assert!(
        prompt_resp.get("error").is_none(),
        "prompt must not return an error: {prompt_resp}"
    );
    assert_eq!(
        prompt_resp["stopReason"].as_str(),
        Some("end_turn"),
        "expected end_turn stop reason: {prompt_resp}"
    );

    // ── 6. collect text chunk notifications ───────────────────────────────────
    let mut chunks: Vec<String> = Vec::new();
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(500), notif_sub.next()).await
    {
        let v: Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
        if v["update"]["sessionUpdate"].as_str() == Some("agent_message_chunk") {
            if let Some(text) = v["update"]["content"]["text"].as_str() {
                chunks.push(text.to_string());
            }
        }
    }

    let full_text = chunks.join("");
    eprintln!("text chunks={chunks:?}  full={full_text:?}");
    assert!(
        !full_text.is_empty(),
        "expected at least one text notification chunk from OpenRouter via NATS"
    );
}
