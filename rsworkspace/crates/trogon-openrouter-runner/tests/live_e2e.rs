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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use async_trait::async_trait;
use agent_client_protocol::{
    Agent, AuthenticateRequest, CloseSessionRequest, ContentBlock, McpServer, McpServerHttp,
    NewSessionRequest, PromptRequest, SessionNotification, SetSessionModelRequest,
};
use futures_util::StreamExt as _;
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
use trogon_openrouter_runner::{
    NatsSessionNotifier, OpenRouterAgent, OpenRouterClient, SessionNotifier,
};

/// No-op session notifier for the in-memory live tests (drops notifications).
struct NoopNotifier;
#[async_trait(?Send)]
impl SessionNotifier for NoopNotifier {
    async fn notify(&self, _notification: SessionNotification) {}
}

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
        if v["update"]["sessionUpdate"].as_str() == Some("agent_message_chunk")
            && let Some(text) = v["update"]["content"]["text"].as_str() {
                chunks.push(text.to_string());
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

async fn do_close_session(nats: &async_nats::Client, session_id: &str) {
    let session_id = session_id.to_string();
    let payload = serde_json::to_vec(&CloseSessionRequest::new(session_id.clone())).unwrap();
    let resp = nats_req(nats, format!("acp.session.{session_id}.agent.close"), payload).await;
    assert!(resp.get("error").is_none(), "close_session must not error: {resp}");
}

async fn do_set_session_model(nats: &async_nats::Client, session_id: &str, model: &str) {
    let session_id = session_id.to_string();
    let model = model.to_string();
    let payload = serde_json::to_vec(&SetSessionModelRequest::new(session_id.clone(), model.clone())).unwrap();
    let resp = nats_req(nats, format!("acp.session.{session_id}.agent.set_model"), payload).await;
    assert!(resp.get("error").is_none(), "set_session_model to {model} must not error: {resp}");
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

/// close_session lifecycle: after closing a session, prompting it returns a not-found error.
///
/// Verifies the session is fully removed from the agent and that subsequent
/// requests are rejected cleanly rather than hanging or panicking.
/// Free test — no tokens consumed after the close.
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn close_session_then_prompt_returns_not_found() {
    let key = test_key();
    let model = std::env::var("OPENROUTER_TEST_MODEL")
        .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());

    let (_container, nats) = start_nats().await;
    start_agent_with_real_openrouter(nats.clone(), key.clone(), &model).await;

    do_initialize(&nats).await;
    do_authenticate_user_key(&nats, &key).await;
    let session_id = do_new_session(&nats).await;
    eprintln!("session_id={session_id}");

    // Close the session.
    do_close_session(&nats, &session_id).await;

    // Prompt the now-closed session — expect a not-found error response.
    let session_id_owned = session_id.clone();
    let text_owned = "hello".to_string();
    let payload = serde_json::to_vec(&PromptRequest::new(
        session_id_owned.clone(),
        vec![ContentBlock::from(text_owned)],
    ))
    .unwrap();
    let subject = format!("acp.session.{session_id}.agent.prompt");
    let resp = nats_req(&nats, subject, payload).await;
    eprintln!("closed-session prompt response: {resp}");
    assert!(
        resp.get("code").is_some(),
        "prompting a closed session must return an error with a code field: {resp}"
    );
    // ResourceNotFound = -32002
    assert_eq!(
        resp["code"].as_i64(),
        Some(-32002),
        "expected ResourceNotFound (-32002): {resp}"
    );
}

/// Usage notifications carry non-zero token counts.
///
/// Verifies that the usage_update NATS notification is published after a
/// successful prompt and that both prompt-token and completion-token counts
/// are greater than zero.
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn usage_notification_reports_nonzero_token_counts() {
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

    let resp = do_prompt(&nats, &session_id, "Reply with exactly one word: yes").await;
    assert_eq!(resp["stopReason"].as_str(), Some("end_turn"), "{resp}");

    // Drain all notifications and find the usage_update.
    let mut usage_used: Option<u64> = None;
    let mut usage_size: Option<u64> = None;
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(500), notif_sub.next()).await
    {
        let v: Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
        if v["update"]["sessionUpdate"].as_str() == Some("usage_update") {
            usage_used = v["update"]["used"].as_u64();
            usage_size = v["update"]["size"].as_u64();
        }
    }

    eprintln!("usage_used={usage_used:?}  usage_size={usage_size:?}");
    assert!(
        usage_used.map(|n| n > 0).unwrap_or(false),
        "usage_update must have used > 0 (prompt tokens)"
    );
    assert!(
        usage_size.map(|n| n > 0).unwrap_or(false),
        "usage_update must have size > 0 (completion tokens)"
    );
}

/// set_session_model: switching the model mid-session takes effect on the next prompt.
///
/// The agent defaults to openai/gpt-4o-mini. This test switches to openai/gpt-4o
/// (which is in the default available-models list) and verifies a subsequent prompt
/// still returns a non-empty text response, proving the model-switch plumbing works.
#[ignore = "requires OPENROUTER_TEST_KEY and Docker — see file header"]
#[tokio::test]
async fn set_session_model_switches_model_for_next_prompt() {
    let key = test_key();
    // Start with the cheap default model; switch to another available model.
    let default_model = std::env::var("OPENROUTER_TEST_MODEL")
        .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    // openai/gpt-4o is always in the default available_models list.
    let target_model = "openai/gpt-4o";

    let (_container, nats) = start_nats().await;
    start_agent_with_real_openrouter(nats.clone(), key.clone(), &default_model).await;

    do_initialize(&nats).await;
    do_authenticate_user_key(&nats, &key).await;
    let session_id = do_new_session(&nats).await;
    eprintln!("session_id={session_id}");

    // Switch model before sending any prompt.
    do_set_session_model(&nats, &session_id, target_model).await;

    let notif_subject = format!("acp.session.{session_id}.client.session.update");
    let mut notif_sub = nats.subscribe(notif_subject).await.expect("subscribe");

    let resp = do_prompt(&nats, &session_id, "Reply with exactly one word: yes").await;
    assert_eq!(resp["stopReason"].as_str(), Some("end_turn"), "{resp}");

    let full_text = collect_text_chunks(&mut notif_sub).await;
    eprintln!("set_session_model full_text={full_text:?}");
    assert!(!full_text.is_empty(), "prompt after set_session_model must return text via NATS");
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

// ── In-memory LIVE tests (no Docker/NATS) — real model via OpenAI-compatible API ──
//
// These drive the OpenRouterAgent's own loop directly against a REAL model and a
// REAL local HTTP MCP server / elicitation+permission channels — exercising the
// MCP, ask_user, and permission code paths end-to-end. The OpenRouter client is
// OpenAI-compatible, so any OpenAI-compatible endpoint works (real OpenRouter, or
// xAI's `https://api.x.ai/v1` with a grok model). Configure via:
//   OR_LIVE_KEY=<key>  OR_LIVE_BASE_URL=<https://api.x.ai/v1|...>  OR_LIVE_MODEL=<grok-3-mini|...>
// Self-skip when OR_LIVE_KEY is unset. Run with --ignored.

fn live_cfg() -> Option<(String, String, String)> {
    let key = std::env::var("OR_LIVE_KEY").ok().filter(|k| !k.is_empty())?;
    let base = std::env::var("OR_LIVE_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());
    let model = std::env::var("OR_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    Some((key, base, model))
}

fn mcp_server() -> httpmock::MockServer {
    let mcp = httpmock::MockServer::start();
    mcp.mock(|when, then| {
        when.method(httpmock::Method::POST).body_contains("\"initialize\"");
        then.status(200)
            .json_body(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}}));
    });
    mcp.mock(|when, then| {
        when.method(httpmock::Method::POST).body_contains("tools/list");
        then.status(200).json_body(serde_json::json!({
            "jsonrpc":"2.0","id":2,
            "result":{"tools":[{
                "name":"search",
                "description":"Search the web for a query and return a short result.",
                "inputSchema":{"type":"object","properties":{
                    "query":{"type":"string","description":"the search query"}
                },"required":["query"]}
            }]}
        }));
    });
    mcp
}

/// LIVE: real model calls an MCP tool (dispatched over real HTTP) and `ask_user`
/// (answer round-trips through the elicitation channel).
#[tokio::test]
#[ignore = "live: requires OR_LIVE_KEY and spends real money"]
async fn live_inmem_mcp_and_ask_user() {
    let Some((key, base, model)) = live_cfg() else {
        eprintln!("SKIP live_inmem_mcp_and_ask_user: OR_LIVE_KEY not set");
        return;
    };

    // ---- MCP ----
    let mcp = mcp_server();
    let call_mock = mcp.mock(|when, then| {
        when.method(httpmock::Method::POST).body_contains("tools/call");
        then.status(200).json_body(serde_json::json!({
            "jsonrpc":"2.0","id":3,
            "result":{"content":[{"type":"text","text":"MCP RESULT: rust is a systems language"}],"isError":false}
        }));
    });
    let agent = OpenRouterAgent::with_deps(
        NoopNotifier,
        model.clone(),
        key.clone(),
        OpenRouterClient::with_base_url(base.clone()),
    );
    let server = McpServer::Http(McpServerHttp::new("web", mcp.url("/mcp")));
    let sid = agent
        .new_session(NewSessionRequest::new("/tmp").mcp_servers(vec![server]))
        .await
        .unwrap()
        .session_id
        .to_string();
    let p1 = "You have a tool named `web__search`. Call it with query \"rust\". \
        You MUST call the web__search tool — do not answer from your own knowledge. \
        After the tool result, reply with one short sentence.";
    let r1 = tokio::time::timeout(
        Duration::from_secs(180),
        agent.prompt(PromptRequest::new(sid, vec![ContentBlock::from(p1)])),
    )
    .await
    .expect("MCP prompt timed out")
    .expect("MCP prompt failed");
    let hits = call_mock.hits();
    eprintln!("LIVE/MCP(openrouter): stop_reason={:?}, tools/call hits={hits}", r1.stop_reason);
    assert!(hits >= 1, "real model did not call the MCP tool web__search (hits=0)");

    // ---- ask_user ----
    let (elic_tx, mut elic_rx) =
        tokio::sync::mpsc::channel::<trogon_runner_tools::ElicitationReq>(8);
    let captured_q = Arc::new(Mutex::new(None::<String>));
    let cq = Arc::clone(&captured_q);
    let responder = tokio::spawn(async move {
        if let Some(req) = elic_rx.recv().await {
            *cq.lock().unwrap() = Some(req.request.message.clone());
            let mut content = std::collections::BTreeMap::new();
            content.insert(
                "answer".to_string(),
                agent_client_protocol::ElicitationContentValue::String("teal".to_string()),
            );
            let resp = agent_client_protocol::ElicitationResponse::new(
                agent_client_protocol::ElicitationAction::Accept(
                    agent_client_protocol::ElicitationAcceptAction::new().content(content),
                ),
            );
            let _ = req.response_tx.send(Ok(resp));
        }
    });
    let agent2 = OpenRouterAgent::with_deps(
        NoopNotifier,
        model,
        key,
        OpenRouterClient::with_base_url(base),
    )
    .with_elicitation(elic_tx);
    let sid2 = agent2
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap()
        .session_id
        .to_string();
    let p2 = "Use the `ask_user` tool to ask me exactly: What is your favorite color? \
        You MUST call the ask_user tool. After I answer, reply telling me the color I chose.";
    let r2 = tokio::time::timeout(
        Duration::from_secs(180),
        agent2.prompt(PromptRequest::new(sid2, vec![ContentBlock::from(p2)])),
    )
    .await
    .expect("ask_user prompt timed out")
    .expect("ask_user prompt failed");
    let _ = responder.await;
    let q = captured_q.lock().unwrap().clone();
    eprintln!("LIVE/ask_user(openrouter): stop_reason={:?}, question={q:?}", r2.stop_reason);
    assert!(q.is_some(), "real model did not call ask_user (no question forwarded)");
}

/// LIVE: in `default` mode an MCP tool requires approval; the real model's call
/// fires a PermissionReq that the test approves, then the tool executes.
#[tokio::test]
#[ignore = "live: requires OR_LIVE_KEY and spends real money"]
async fn live_inmem_permission_round_trip() {
    let Some((key, base, model)) = live_cfg() else {
        eprintln!("SKIP live_inmem_permission_round_trip: OR_LIVE_KEY not set");
        return;
    };
    let mcp = mcp_server();
    let call_mock = mcp.mock(|when, then| {
        when.method(httpmock::Method::POST).body_contains("tools/call");
        then.status(200).json_body(serde_json::json!({
            "jsonrpc":"2.0","id":3,
            "result":{"content":[{"type":"text","text":"MCP RESULT: rust"}],"isError":false}
        }));
    });

    let (perm_tx, mut perm_rx) =
        tokio::sync::mpsc::channel::<trogon_runner_tools::PermissionReq>(8);
    let requested = Arc::new(Mutex::new(Vec::<String>::new()));
    let rq = Arc::clone(&requested);
    let approver = tokio::spawn(async move {
        while let Some(req) = perm_rx.recv().await {
            rq.lock().unwrap().push(req.tool_name.clone());
            let _ = req.started_tx.send(());
            let _ = req.response_tx.send(true);
        }
    });

    let agent = OpenRouterAgent::with_deps(
        NoopNotifier,
        model,
        key,
        OpenRouterClient::with_base_url(base),
    )
    .with_permission_gate(perm_tx, trogon_runner_tools::AllowedToolsSessionStore::new());
    let server = McpServer::Http(McpServerHttp::new("web", mcp.url("/mcp")));
    let sid = agent
        .new_session(NewSessionRequest::new("/tmp").mcp_servers(vec![server]))
        .await
        .unwrap()
        .session_id
        .to_string();
    let p = "You have a tool named `web__search`. Call it with query \"rust\". \
        You MUST call the web__search tool. After the result, reply with one short sentence.";
    let r = tokio::time::timeout(
        Duration::from_secs(180),
        agent.prompt(PromptRequest::new(sid, vec![ContentBlock::from(p)])),
    )
    .await
    .expect("permission prompt timed out")
    .expect("permission prompt failed");
    approver.abort();
    let reqs = requested.lock().unwrap().clone();
    let hits = call_mock.hits();
    eprintln!(
        "LIVE/permission(openrouter): stop_reason={:?}, approvals={reqs:?}, tools/call hits={hits}",
        r.stop_reason
    );
    assert!(
        reqs.iter().any(|t| t == "web__search"),
        "permission approval must be requested for web__search; got {reqs:?}"
    );
    assert!(hits >= 1, "MCP tool must execute after approval");
}
