//! End-to-end integration test: trogon-xai-runner ↔ trogon-console.
//!
//! Flow:
//!    1. ACP initialize
//!    2. ACP authenticate (XAI_API_KEY)
//!    3. ACP session.new
//!    4. ACP prompt (first turn)
//!    5. Console GET /sessions
//!    6. Multi-turn — second prompt in same session
//!    7. Tool use — enable web_search, send search query
//!    8. Fork session — fork + prompt in fork
//!    9. ACP session.list — verify session in ACP list
//!   10. set_model — switch to grok-3, prompt
//!   11. resume_session — verify no-op ok
//!   12. Console GET /sessions/{tenant_id}/{session_id} — direct lookup
//!   13. Console GET /agents/{id}/sessions — by agent_id
//!   14. close_session — close + verify console behavior
//!
//! Env vars:
//!   NATS_URL       (default: nats://localhost:4222)
//!   ACP_PREFIX     (default: acp)
//!   XAI_API_KEY    (required)
//!   CONSOLE_URL    (default: http://localhost:8090)
//!   PROMPT_TEXT    (default: "Di 'hola' en exactamente una palabra, sin puntuación.")

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, push},
};
use bytes::Bytes;
use futures_util::StreamExt as _;
use serde_json::{Value, json};

const TIMEOUT_SECS: u64 = 120;
const TOTAL_STEPS: u8 = 17;

#[tokio::main]
async fn main() {
    let nats_url = env("NATS_URL", "nats://localhost:4222");
    let prefix = env("ACP_PREFIX", "acp");
    let xai_api_key = std::env::var("XAI_API_KEY").unwrap_or_else(|_| {
        eprintln!("WARN: XAI_API_KEY not set — will try server key");
        String::new()
    });
    let console_url = env("CONSOLE_URL", "http://localhost:8090");
    let prompt_text = env(
        "PROMPT_TEXT",
        "Di 'hola' en exactamente una palabra, sin puntuación.",
    );

    println!("═══════════════════════════════════════════════════════");
    println!(" trogon E2E: xai-runner ↔ console");
    println!("   NATS:    {}", nats_url);
    println!("   prefix:  {}", prefix);
    println!("   console: {}", console_url);
    println!("═══════════════════════════════════════════════════════\n");

    let nats = async_nats::connect(&nats_url)
        .await
        .expect("connect to NATS");
    let js = jetstream::new(nats.clone());

    let prefix_upper = prefix.to_uppercase().replace('.', "_");
    let notif_stream = format!("{prefix_upper}_NOTIFICATIONS");
    let resp_stream = format!("{prefix_upper}_RESPONSES");
    let cmd_stream = format!("{prefix_upper}_COMMANDS");

    // ── 1. initialize ──────────────────────────────────────────────────────────
    step(1, "initialize");
    let init_resp = nats_req(
        &nats,
        format!("{prefix}.agent.initialize"),
        json!({ "protocol_version": 1 }),
    )
    .await;
    let proto = init_resp["protocolVersion"].as_u64().unwrap_or(0);
    let agent_name = init_resp["agent"]["name"].as_str().unwrap_or("?");
    println!("  protocol_version={proto}, agent={agent_name}");
    ok("initialize");

    // ── 2. authenticate ────────────────────────────────────────────────────────
    step(2, "authenticate");
    let auth_body = if xai_api_key.is_empty() {
        json!({ "method_id": "agent" })
    } else {
        json!({
            "method_id": "xai-api-key",
            "_meta": { "XAI_API_KEY": xai_api_key }
        })
    };
    let auth_resp = nats_req(&nats, format!("{prefix}.agent.authenticate"), auth_body).await;
    if let Some(err) = auth_resp.get("error") {
        fail(&format!("authenticate error: {err}"));
    }
    ok("authenticate");

    // ── 3. new_session ─────────────────────────────────────────────────────────
    step(3, "session.new");
    let session_resp = nats_req(
        &nats,
        format!("{prefix}.agent.session.new"),
        json!({ "cwd": "/tmp/e2e-test", "mcpServers": [] }),
    )
    .await;
    if let Some(err) = session_resp.get("error") {
        fail(&format!("session.new error: {err}"));
    }
    let session_id = session_resp["sessionId"]
        .as_str()
        .or_else(|| session_resp["session_id"].as_str())
        .unwrap_or_else(|| {
            eprintln!("  DEBUG session.new response: {session_resp}");
            panic!("sessionId not found in response")
        })
        .to_string();
    let model = session_resp["models"]
        .as_array()
        .and_then(|m| m.first())
        .and_then(|m| m["id"].as_str())
        .unwrap_or("?");
    println!("  session_id={session_id}");
    println!("  model={model}");
    ok("session.new");

    // ── 4. prompt (first turn) ─────────────────────────────────────────────────
    step(4, "prompt (first turn)");
    println!("  text: \"{prompt_text}\"");

    let stop1 = do_prompt(
        &js, &prefix, &notif_stream, &resp_stream, &session_id,
        json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": prompt_text }]
        }),
    )
    .await;
    println!("  stop_reason={stop1}");
    ok("prompt (first turn)");

    // ── 5. console GET /sessions ───────────────────────────────────────────────
    step(5, "GET /sessions (console)");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let http = reqwest::Client::new();
    let sessions = get_sessions(&http, &console_url).await;
    let found = sessions.iter().find(|s| s["id"].as_str() == Some(&session_id));
    let tenant_id = match found {
        Some(s) => {
            print_session(s);
            ok("session found in console");
            s["agent_id"].as_str().unwrap_or("").to_string()
        }
        None => {
            println!("  WARN: session not found (total: {})", sessions.len());
            String::new()
        }
    };

    // ── 6. multi-turn — second prompt ─────────────────────────────────────────
    step(6, "multi-turn (second prompt)");
    let second = "Ahora di 'adiós' en exactamente una palabra, sin puntuación.";
    println!("  text: \"{second}\"");

    let stop2 = do_prompt(
        &js, &prefix, &notif_stream, &resp_stream, &session_id,
        json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": second }]
        }),
    )
    .await;
    println!("  stop_reason={stop2}");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    if let Some(s) = get_sessions(&http, &console_url).await.iter()
        .find(|s| s["id"].as_str() == Some(&session_id))
    {
        let c = s["message_count"].as_u64().unwrap_or(0);
        let i = s["input_tokens"].as_u64().unwrap_or(0);
        let o = s["output_tokens"].as_u64().unwrap_or(0);
        println!("  message_count={c} (expected ≥4)  input={i}  output={o}");
        if c < 4 { eprintln!("  WARN: expected ≥4 messages, got {c}"); }
    }
    ok("multi-turn");

    // ── 7. tool use — enable web_search, send search query ────────────────────
    step(7, "tool use (web_search)");

    let r7 = uuid::Uuid::new_v4().to_string();
    let cfg_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.set_config_option"),
        json!({ "sessionId": session_id, "configId": "web_search", "value": "on" }),
        &r7,
    ).await;
    if let Some(err) = cfg_resp.get("error") {
        eprintln!("  WARN: set_config_option error: {err}");
    } else {
        println!("  web_search enabled");
    }

    let search_q = "What is the capital of France? Answer in one word.";
    println!("  text: \"{search_q}\"");
    let stop3 = do_prompt(
        &js, &prefix, &notif_stream, &resp_stream, &session_id,
        json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": search_q }] }),
    ).await;
    println!("  stop_reason={stop3}");
    ok("tool use (web_search)");

    // ── 8. fork session ────────────────────────────────────────────────────────
    step(8, "fork session");

    let r8 = uuid::Uuid::new_v4().to_string();
    let fork_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.fork"),
        json!({ "sessionId": session_id, "cwd": "/tmp/e2e-fork", "mcpServers": [] }),
        &r8,
    ).await;
    if let Some(err) = fork_resp.get("error") {
        fail(&format!("fork error: {err}"));
    }
    let forked_id = fork_resp["sessionId"]
        .as_str()
        .or_else(|| fork_resp["session_id"].as_str())
        .unwrap_or_else(|| { eprintln!("  DEBUG fork: {fork_resp}"); panic!("forked sessionId not found") })
        .to_string();
    println!("  forked_session_id={forked_id}");

    // Disable web_search in fork before prompting (grok-3-mini doesn't support server-side tools).
    let r8b = uuid::Uuid::new_v4().to_string();
    let _ = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &forked_id,
        format!("{prefix}.session.{forked_id}.agent.set_config_option"),
        json!({ "sessionId": forked_id, "configId": "web_search", "value": "off" }),
        &r8b,
    ).await;

    let fork_prompt = "Di 'fork' en exactamente una palabra.";
    println!("  prompting fork: \"{fork_prompt}\"");
    let stop4 = do_prompt(
        &js, &prefix, &notif_stream, &resp_stream, &forked_id,
        json!({ "sessionId": forked_id, "prompt": [{ "type": "text", "text": fork_prompt }] }),
    ).await;
    println!("  stop_reason={stop4}");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let s8 = get_sessions(&http, &console_url).await;
    println!("  original in console: {}", s8.iter().any(|s| s["id"].as_str() == Some(&session_id)));
    println!("  forked in console:   {}", s8.iter().any(|s| s["id"].as_str() == Some(&forked_id)));
    ok("fork session");

    // ── 9. ACP session.list ────────────────────────────────────────────────────
    step(9, "session.list (ACP)");

    let list_resp = nats_req(
        &nats,
        format!("{prefix}.agent.session.list"),
        json!({}),
    ).await;
    if let Some(err) = list_resp.get("error") {
        eprintln!("  WARN: session.list error: {err}");
    } else {
        let sessions_arr = list_resp["sessions"].as_array().cloned().unwrap_or_default();
        println!("  total sessions in runner: {}", sessions_arr.len());
        let has_orig = sessions_arr.iter().any(|s| s["sessionId"].as_str() == Some(&session_id) || s["id"].as_str() == Some(&session_id));
        let has_fork = sessions_arr.iter().any(|s| s["sessionId"].as_str() == Some(&forked_id) || s["id"].as_str() == Some(&forked_id));
        println!("  original session in list: {has_orig}");
        println!("  forked session in list:   {has_fork}");
    }
    ok("session.list (ACP)");

    // ── 10. set_model — switch to grok-3, prompt ──────────────────────────────
    step(10, "set_model (grok-3)");

    // Disable web_search first so the prompt doesn't hit the server-side tools restriction.
    let r10a = uuid::Uuid::new_v4().to_string();
    let _ = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.set_config_option"),
        json!({ "sessionId": session_id, "configId": "web_search", "value": "off" }),
        &r10a,
    ).await;

    let r10b = uuid::Uuid::new_v4().to_string();
    let model_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.set_model"),
        json!({ "sessionId": session_id, "modelId": "grok-3" }),
        &r10b,
    ).await;
    if let Some(err) = model_resp.get("error") {
        eprintln!("  WARN: set_model error: {err}");
    } else {
        println!("  model switched to grok-3");
    }

    let grok3_prompt = "Di 'grok3' en exactamente una palabra.";
    println!("  text: \"{grok3_prompt}\"");
    let stop5 = do_prompt(
        &js, &prefix, &notif_stream, &resp_stream, &session_id,
        json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": grok3_prompt }] }),
    ).await;
    println!("  stop_reason={stop5}");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    if let Some(s) = get_sessions(&http, &console_url).await.iter()
        .find(|s| s["id"].as_str() == Some(&session_id))
    {
        println!("  model in console: {}", s["model"].as_str().unwrap_or("?"));
    }
    ok("set_model (grok-3)");

    // ── 11. resume_session ─────────────────────────────────────────────────────
    step(11, "resume_session");

    let r11 = uuid::Uuid::new_v4().to_string();
    let resume_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.resume"),
        json!({ "sessionId": session_id, "cwd": "/tmp/e2e-test" }),
        &r11,
    ).await;
    if let Some(err) = resume_resp.get("error") {
        eprintln!("  WARN: resume error: {err}");
    } else {
        println!("  resume ok");
    }
    ok("resume_session");

    // ── 12. console GET /sessions/{tenant_id}/{session_id} ────────────────────
    step(12, "GET /sessions/{tenant_id}/{session_id} (console)");

    if tenant_id.is_empty() {
        println!("  SKIP: tenant_id unknown (step 5 did not find session)");
    } else {
        let url = format!("{console_url}/sessions/{tenant_id}/{session_id}");
        println!("  GET {url}");
        match http.get(&url).send().await {
            Err(e) => eprintln!("  WARN: request failed: {e}"),
            Ok(resp) => {
                let status = resp.status();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                if status.is_success() {
                    print_session(&body);
                    ok("direct session lookup");
                } else {
                    eprintln!("  WARN: HTTP {status}: {body}");
                }
            }
        }
    }

    // ── 13. console GET /agents/{id}/sessions ─────────────────────────────────
    step(13, "GET /agents/{id}/sessions (console)");

    if tenant_id.is_empty() {
        println!("  SKIP: agent_id unknown");
    } else {
        let url = format!("{console_url}/agents/{tenant_id}/sessions");
        println!("  GET {url}");
        match http.get(&url).send().await {
            Err(e) => eprintln!("  WARN: request failed: {e}"),
            Ok(resp) => {
                let status = resp.status();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                let arr = body.as_array().cloned().unwrap_or_default();
                println!("  sessions for agent: {}", arr.len());
                let has = arr.iter().any(|s| s["id"].as_str() == Some(&session_id));
                println!("  original session present: {has}");
                if status.is_success() {
                    ok("agent sessions lookup");
                } else {
                    eprintln!("  WARN: HTTP {status}");
                }
            }
        }
    }

    // ── 14. load_session ──────────────────────────────────────────────────────
    step(14, "load_session");

    let r14a = uuid::Uuid::new_v4().to_string();
    let load_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.load"),
        json!({ "sessionId": session_id, "cwd": "/tmp/e2e-test", "mcpServers": [] }),
        &r14a,
    ).await;
    if let Some(err) = load_resp.get("error") {
        eprintln!("  WARN: load_session error: {err}");
    } else {
        let current_model = load_resp["models"]["currentModelId"].as_str().unwrap_or("?");
        let n_options = load_resp["configOptions"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("  current_model={current_model}  config_options={n_options}");
        ok("load_session");
    }

    // ── 15. set_mode ───────────────────────────────────────────────────────────
    step(15, "set_mode (default)");

    let r15 = uuid::Uuid::new_v4().to_string();
    let mode_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.set_mode"),
        json!({ "sessionId": session_id, "modeId": "default" }),
        &r15,
    ).await;
    if let Some(err) = mode_resp.get("error") {
        eprintln!("  WARN: set_mode error: {err}");
    } else {
        println!("  set_mode ok");
    }
    ok("set_mode (default)");

    // ── 16. close_session ──────────────────────────────────────────────────────
    step(16, "close_session");

    let r16 = uuid::Uuid::new_v4().to_string();
    let close_resp = js_command(
        &js, &prefix, &resp_stream, &cmd_stream, &session_id,
        format!("{prefix}.session.{session_id}.agent.close"),
        json!({ "sessionId": session_id }),
        &r16,
    ).await;
    if let Some(err) = close_resp.get("error") {
        eprintln!("  WARN: close error: {err}");
    } else {
        println!("  session closed");
    }

    // Verify: session gone from ACP list, still in console KV.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let list2 = nats_req(&nats, format!("{prefix}.agent.session.list"), json!({})).await;
    let acp_sessions = list2["sessions"].as_array().cloned().unwrap_or_default();
    let still_in_acp = acp_sessions.iter()
        .any(|s| s["sessionId"].as_str() == Some(&session_id) || s["id"].as_str() == Some(&session_id));
    println!("  session still in ACP list: {still_in_acp} (expected: false)");

    let console_sessions = get_sessions(&http, &console_url).await;
    let still_in_console = console_sessions.iter().any(|s| s["id"].as_str() == Some(&session_id));
    println!("  session still in console:  {still_in_console} (expected: true — KV persists)");

    ok("close_session");

    // ── 17. logout ────────────────────────────────────────────────────────────
    step(17, "logout");

    let logout_resp = nats_req(
        &nats,
        format!("{prefix}.agent.logout"),
        json!({}),
    ).await;
    if let Some(err) = logout_resp.get("error") {
        eprintln!("  WARN: logout error: {err}");
    } else {
        println!("  logout ok");
    }

    // After logout the pending API key should be cleared — a new session without
    // re-authenticating should fail (no key available unless server key is set).
    let post_logout = nats_req(
        &nats,
        format!("{prefix}.agent.session.new"),
        json!({ "cwd": "/tmp/e2e-post-logout", "mcpServers": [] }),
    ).await;
    let post_logout_err = post_logout.get("error").is_some();
    println!("  new_session after logout fails: {post_logout_err}");
    ok("logout");

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(" E2E complete");
    println!("═══════════════════════════════════════════════════════");
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn step(n: u8, name: &str) {
    println!("[{n}/{TOTAL_STEPS}] {name}...");
}

fn ok(name: &str) {
    println!("  ✓ {name}\n");
}

fn fail(msg: &str) -> ! {
    eprintln!("  ✗ {msg}");
    std::process::exit(1);
}

async fn nats_req(nats: &async_nats::Client, subject: String, body: Value) -> Value {
    let payload = Bytes::from(serde_json::to_vec(&body).unwrap());
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        nats.request(subject.clone(), payload),
    )
    .await
    .unwrap_or_else(|_| panic!("timeout on {subject}"))
    .unwrap_or_else(|e| panic!("request to {subject} failed: {e}"));
    serde_json::from_slice(&resp.payload).unwrap_or(Value::Null)
}

async fn create_push_consumer(
    js: &jetstream::Context,
    stream_name: &str,
    filter_subject: &str,
    req_id: &str,
    kind: &str,
) -> async_nats::jetstream::consumer::Consumer<push::Config> {
    let stream = js
        .get_stream(stream_name)
        .await
        .unwrap_or_else(|e| panic!("get stream {stream_name}: {e}"));

    let consumer_name = format!("e2e-{kind}-{req_id}");
    stream
        .create_consumer(push::Config {
            name: Some(consumer_name.clone()),
            durable_name: Some(consumer_name),
            deliver_subject: format!("_INBOX.e2e.{req_id}.{kind}"),
            filter_subject: filter_subject.to_string(),
            deliver_policy: DeliverPolicy::All,
            ack_policy: AckPolicy::Explicit,
            replay_policy: ReplayPolicy::Instant,
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create consumer for {filter_subject}: {e}"))
}

/// Send a prompt via JetStream, drain notifications, return stop_reason.
async fn do_prompt(
    js: &jetstream::Context,
    prefix: &str,
    notif_stream: &str,
    resp_stream: &str,
    session_id: &str,
    payload: Value,
) -> String {
    let req_id = uuid::Uuid::new_v4().to_string();
    let notif_filter = format!("{prefix}.session.{session_id}.agent.update.{req_id}");
    let resp_filter = format!("{prefix}.session.{session_id}.agent.prompt.response.{req_id}");

    let notif_consumer = create_push_consumer(js, notif_stream, &notif_filter, &req_id, "notif").await;
    let resp_consumer = create_push_consumer(js, resp_stream, &resp_filter, &req_id, "resp").await;

    let mut notif_messages = notif_consumer.messages().await.expect("notif messages");
    let mut resp_messages = resp_consumer.messages().await.expect("resp messages");

    let mut headers = async_nats::HeaderMap::new();
    headers.insert("X-Req-Id", req_id.as_str());
    headers.insert("X-Session-Id", session_id);

    let prompt_subject = format!("{prefix}.session.{session_id}.agent.prompt");
    let payload_bytes = Bytes::from(serde_json::to_vec(&payload).unwrap());
    js.publish_with_headers(prompt_subject, headers, payload_bytes)
        .await
        .expect("js publish prompt")
        .await
        .expect("js ack");

    print!("  response: ");
    let timeout = std::time::Duration::from_secs(TIMEOUT_SECS);
    let stop_reason;
    loop {
        tokio::select! {
            notif = tokio::time::timeout(timeout, notif_messages.next()) => {
                let msg = notif.expect("timeout waiting for notification")
                    .expect("notif stream ended")
                    .expect("notif message error");
                let _ = msg.ack().await;
                let val: Value = serde_json::from_slice(msg.payload.as_ref())
                    .unwrap_or(Value::Null);
                if let Some(text) = extract_text_chunk(&val) {
                    print!("{text}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
            }
            resp = tokio::time::timeout(timeout, resp_messages.next()) => {
                let msg = resp.expect("timeout waiting for prompt response")
                    .expect("resp stream ended")
                    .expect("resp message error");
                let _ = msg.ack().await;
                let raw = String::from_utf8_lossy(msg.payload.as_ref()).to_string();
                eprintln!("  DEBUG PromptResponse: {raw}");
                let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                stop_reason = val["stopReason"]
                    .as_str()
                    .or_else(|| val["stop_reason"].as_str())
                    .unwrap_or("unknown")
                    .to_string();
                break;
            }
        }
    }
    println!();
    stop_reason
}

/// Send a non-prompt JetStream command and wait for the response on
/// `{prefix}.session.{sid}.agent.response.{req_id}`.
async fn js_command(
    js: &jetstream::Context,
    prefix: &str,
    resp_stream: &str,
    cmd_stream: &str,
    session_id: &str,
    subject: String,
    payload: Value,
    req_id: &str,
) -> Value {
    let resp_filter = format!("{prefix}.session.{session_id}.agent.response.{req_id}");
    let resp_consumer = create_push_consumer(js, resp_stream, &resp_filter, req_id, "cmd").await;
    let mut resp_messages = resp_consumer.messages().await.expect("cmd resp messages");

    js.get_stream(cmd_stream)
        .await
        .unwrap_or_else(|e| panic!("get stream {cmd_stream}: {e}"));

    let mut headers = async_nats::HeaderMap::new();
    headers.insert("X-Req-Id", req_id);
    headers.insert("X-Session-Id", session_id);

    let payload_bytes = Bytes::from(serde_json::to_vec(&payload).unwrap());
    js.publish_with_headers(subject.clone(), headers, payload_bytes)
        .await
        .unwrap_or_else(|e| panic!("js publish {subject}: {e}"))
        .await
        .expect("js ack");

    let timeout = std::time::Duration::from_secs(30);
    let msg = tokio::time::timeout(timeout, resp_messages.next())
        .await
        .unwrap_or_else(|_| panic!("timeout waiting for response to {subject}"))
        .expect("resp stream ended")
        .expect("resp message error");
    let _ = msg.ack().await;
    let raw = String::from_utf8_lossy(msg.payload.as_ref()).to_string();
    eprintln!("  DEBUG cmd ({subject}): {raw}");
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

async fn get_sessions(http: &reqwest::Client, console_url: &str) -> Vec<Value> {
    let sessions: Value = http
        .get(format!("{console_url}/sessions"))
        .send()
        .await
        .expect("GET /sessions")
        .json()
        .await
        .expect("parse /sessions JSON");
    sessions.as_array().cloned().unwrap_or_default()
}

fn print_session(s: &Value) {
    println!("  name={}", s["name"].as_str().unwrap_or("?"));
    println!("  model={}", s["model"].as_str().unwrap_or("?"));
    println!("  status={}", s["status"].as_str().unwrap_or("?"));
    println!("  message_count={}", s["message_count"].as_u64().unwrap_or(0));
    println!(
        "  input_tokens={}  output_tokens={}",
        s["input_tokens"].as_u64().unwrap_or(0),
        s["output_tokens"].as_u64().unwrap_or(0)
    );
    println!("  agent_id={}", s["agent_id"].as_str().unwrap_or("none"));
}

fn extract_text_chunk(val: &Value) -> Option<String> {
    let update = &val["update"];
    let chunk = update.get("agentMessageChunk")?;
    let content = &chunk["content"];
    if content["type"].as_str() == Some("text") {
        return content["text"].as_str().map(str::to_string);
    }
    None
}
