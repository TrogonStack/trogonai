//! Integration tests for `KvSessionStore` against a real NATS server.
//!
//! These tests require a NATS server with JetStream enabled.
//! Set `NATS_TEST_URL` to run them, e.g.:
//!
//!   NATS_TEST_URL=nats://localhost:4222 cargo test -p trogon-xai-runner \
//!     --features test-helpers --test kv_integration
//!
//! Each test creates its own uniquely named KV bucket and deletes it on exit,
//! so tests can run in parallel safely.

use std::sync::{Arc, Mutex};

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    Agent, AuthenticateRequest, CloseSessionRequest, ContentBlock, ForkSessionRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    SetSessionConfigOptionRequest, SetSessionModelRequest, TextContent,
};
use futures_util::StreamExt as _;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use trogon_xai_runner::Message;
use trogon_xai_runner::XaiAgent;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Returns the NATS URL from `NATS_TEST_URL`, or `None` to skip the test.
fn nats_url() -> Option<String> {
    std::env::var("NATS_TEST_URL").ok()
}

/// Connect to a real NATS server.
async fn real_nats(url: &str) -> async_nats::Client {
    async_nats::connect(url).await.expect("connect to NATS")
}

/// A unique bucket name for this test run.
fn unique_bucket() -> String {
    format!("XAITEST{}", uuid::Uuid::new_v4().simple())
}

/// Delete the KV bucket at the end of a test.
async fn cleanup(url: &str, bucket: &str) {
    if let Ok(nats) = async_nats::connect(url).await {
        let js = async_nats::jetstream::new(nats);
        js.delete_key_value(bucket).await.ok();
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Session created by one agent instance is visible to a second instance
/// sharing the same KV bucket.
#[tokio::test]
async fn kv_session_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/workspace"))
        .await
        .unwrap();
    let session_id = sess.session_id.clone();

    // Second agent, same bucket — should see the session.
    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    agent_b
        .load_session(LoadSessionRequest::new(session_id, "/workspace"))
        .await
        .unwrap();

    cleanup(&url, &bucket).await;
}

/// Model change made through one agent is visible to another.
#[tokio::test]
async fn kv_model_change_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.clone();

    agent_a
        .set_session_model(SetSessionModelRequest::new(
            session_id.to_string(),
            "grok-3-mini",
        ))
        .await
        .unwrap();

    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    let model = agent_b.test_session_model(&session_id.to_string()).await;
    assert_eq!(model.as_deref(), Some("grok-3-mini"));

    cleanup(&url, &bucket).await;
}

/// Closing a session on one agent removes it from KV — the other can no
/// longer load it.
#[tokio::test]
async fn kv_close_session_removes_from_kv() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.clone();

    agent_a
        .close_session(CloseSessionRequest::new(session_id.to_string()))
        .await
        .unwrap();

    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    agent_b
        .load_session(LoadSessionRequest::new(session_id, "/tmp"))
        .await
        .unwrap_err(); // session should be gone

    cleanup(&url, &bucket).await;
}

/// `list_sessions` across two agents sharing a bucket returns all sessions.
#[tokio::test]
async fn kv_list_sessions_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();
    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    agent_a
        .new_session(NewSessionRequest::new("/a"))
        .await
        .unwrap();
    agent_b
        .new_session(NewSessionRequest::new("/b"))
        .await
        .unwrap();

    // Each agent should see both sessions when listing.
    let list_a = agent_a
        .list_sessions(agent_client_protocol::ListSessionsRequest::new())
        .await
        .unwrap();
    let list_b = agent_b
        .list_sessions(agent_client_protocol::ListSessionsRequest::new())
        .await
        .unwrap();

    assert_eq!(list_a.sessions.len(), 2);
    assert_eq!(list_b.sessions.len(), 2);

    cleanup(&url, &bucket).await;
}

/// A forked session is persisted in KV and visible to a second agent.
#[tokio::test]
async fn kv_forked_session_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/src"))
        .await
        .unwrap();
    agent_a
        .set_session_model(SetSessionModelRequest::new(
            sess.session_id.to_string(),
            "grok-3-mini",
        ))
        .await
        .unwrap();

    let fork = agent_a
        .fork_session(ForkSessionRequest::new(sess.session_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    // Fork should exist and inherit the model.
    let model = agent_b.test_session_model(&fork_id).await;
    assert_eq!(model.as_deref(), Some("grok-3-mini"));

    cleanup(&url, &bucket).await;
}

/// Tool config (`enabled_tools`) written by one agent survives an agent restart
/// and is visible to a new instance reading the same KV bucket.
#[tokio::test]
async fn kv_tool_config_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    // Enable two server-side tools via agent A.
    agent_a
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();
    agent_a
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "x_search",
            "on",
        ))
        .await
        .unwrap();

    // Agent B opens the same bucket ("restart") — must see both tools enabled.
    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    let tools = agent_b.test_session_enabled_tools(&sid).await;
    assert!(
        tools.contains(&"web_search".to_string()),
        "web_search must persist: {tools:?}"
    );
    assert!(
        tools.contains(&"x_search".to_string()),
        "x_search must persist: {tools:?}"
    );
    assert_eq!(tools.len(), 2, "exactly 2 tools must be enabled: {tools:?}");

    cleanup(&url, &bucket).await;
}

/// `last_response_id` written by one agent survives an agent restart and is
/// visible to a new instance. This is the stateful multi-turn field: without
/// it the new instance would fall back to sending the full history.
#[tokio::test]
async fn kv_last_response_id_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(nats_a, prefix.clone(), "grok-3", "key", &bucket)
        .await
        .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    // Simulate what prompt() stores after a successful xAI turn.
    agent_a
        .test_set_last_response_id(&sid, Some("resp_abc123".to_string()))
        .await;

    // Agent B opens the same bucket ("restart") — must read back the response ID
    // so it can resume stateful multi-turn without re-sending full history.
    let agent_b = XaiAgent::new_with_kv_bucket(nats_b, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    let resp_id = agent_b.test_session_last_response_id(&sid).await;
    assert_eq!(
        resp_id.as_deref(),
        Some("resp_abc123"),
        "last_response_id must survive agent restart: {resp_id:?}",
    );

    cleanup(&url, &bucket).await;
}

/// Sessions created with a short TTL must disappear from `list_sessions` and
/// become un-loadable once the TTL elapses. Verifies that the KV `max_age`
/// config is wired up correctly.
#[tokio::test]
async fn kv_session_expires_after_ttl() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    // 1-second TTL bucket.
    let ttl = std::time::Duration::from_secs(1);
    let agent_a =
        XaiAgent::new_with_kv_bucket_ttl(nats_a, prefix.clone(), "grok-3", "key", &bucket, ttl)
            .await
            .unwrap();

    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    // Session must be visible immediately.
    let before = agent_a
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert_eq!(
        before.sessions.len(),
        1,
        "session must exist before TTL expires"
    );

    // Wait comfortably past the 1-second TTL.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // A new agent instance opens the same bucket — the expired entry must be gone.
    let agent_b = XaiAgent::new_with_kv_bucket_ttl(nats_b, prefix, "grok-3", "key", &bucket, ttl)
        .await
        .unwrap();

    let after = agent_b
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert!(
        after.sessions.is_empty(),
        "session must be gone after TTL: {after:?}"
    );

    let load_result = agent_b
        .load_session(LoadSessionRequest::new(sid.clone(), "/tmp"))
        .await;
    assert!(
        load_result.is_err(),
        "loading an expired session must return an error"
    );

    cleanup(&url, &bucket).await;
}

// ── fake HTTP helpers for KV prompt tests ─────────────────────────────────────

/// Reads past HTTP headers; returns the `Content-Length` value.
async fn drain_request_kv(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> usize {
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.is_err() {
            break;
        }
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.to_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    content_length
}

/// One-shot SSE server: sends `text_chunks` as Responses API `message.delta`
/// events followed by `[DONE]`. Returns the base URL.
async fn fake_xai_sse_kv(response_id: &'static str, text_chunks: &[&'static str]) -> String {
    let body: String = text_chunks
        .iter()
        .map(|t| {
            format!(
                "data: {{\"id\":\"{response_id}\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"
            )
        })
        .chain(std::iter::once("data: [DONE]\n\n".to_string()))
        .collect();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request_kv(&mut reader).await;

            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });

    format!("http://127.0.0.1:{port}/v1")
}

/// Two-connection recording server. Accepts two sequential HTTP connections,
/// reads and stores the JSON request body for each, then responds with SSE.
/// The first response carries `resp_kv_0`, the second `resp_kv_1`.
async fn fake_xai_recording_two(
    chunks_0: Vec<&'static str>,
    chunks_1: Vec<&'static str>,
) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let bodies_clone = Arc::clone(&bodies);

    let all_chunks: Vec<(Vec<&'static str>, &'static str)> =
        vec![(chunks_0, "resp_kv_0"), (chunks_1, "resp_kv_1")];

    tokio::spawn(async move {
        for (chunks, resp_id) in all_chunks {
            if let Ok((stream, _)) = listener.accept().await {
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);

                let content_length = drain_request_kv(&mut reader).await;
                let body_json = if content_length > 0 {
                    let mut body = vec![0u8; content_length];
                    reader.read_exact(&mut body).await.ok();
                    serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                };
                bodies_clone.lock().unwrap().push(body_json);

                let sse_body: String = chunks
                    .iter()
                    .map(|t| {
                        format!(
                            "data: {{\"id\":\"{resp_id}\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"
                        )
                    })
                    .chain(std::iter::once("data: [DONE]\n\n".to_string()))
                    .collect();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    sse_body.len(),
                    sse_body,
                );
                writer.write_all(http.as_bytes()).await.ok();
            }
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), bodies)
}

// ── prompt + KV tests ─────────────────────────────────────────────────────────

/// `prompt()` persists history and `last_response_id` to the real KV bucket.
#[tokio::test]
async fn kv_prompt_persists_history_and_response_id() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();
    let xai_url = fake_xai_sse_kv("resp_kv_0", &["Hello from KV"]).await;

    let agent_a = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_a,
        prefix.clone(),
        "grok-3",
        "key",
        &bucket,
        &xai_url,
    )
    .await
    .unwrap();
    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();
    agent_a
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("ping"))],
        ))
        .await
        .unwrap();

    let agent_b = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_b,
        prefix,
        "grok-3",
        "key",
        &bucket,
        "http://localhost:1",
    )
    .await
    .unwrap();
    let history = agent_b.test_session_history(&sid).await;
    assert_eq!(history.len(), 2, "history must persist to KV: {history:?}");
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
    let resp_id = agent_b.test_session_last_response_id(&sid).await;
    assert_eq!(
        resp_id.as_deref(),
        Some("resp_kv_0"),
        "last_response_id must persist: {resp_id:?}"
    );

    cleanup(&url, &bucket).await;
}

/// Stateful multi-turn: agent B (simulating restart) reads `last_response_id`
/// from KV and sends `previous_response_id` in the second HTTP request.
#[tokio::test]
async fn kv_stateful_multi_turn_uses_previous_response_id() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();
    let (xai_url, bodies) = fake_xai_recording_two(vec!["First reply"], vec!["Second reply"]).await;

    let agent_a = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_a,
        prefix.clone(),
        "grok-3",
        "key",
        &bucket,
        &xai_url,
    )
    .await
    .unwrap();
    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();
    agent_a
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("first"))],
        ))
        .await
        .unwrap();

    let resp_id = agent_a.test_session_last_response_id(&sid).await;
    assert_eq!(
        resp_id.as_deref(),
        Some("resp_kv_0"),
        "resp_id after turn 1: {resp_id:?}"
    );

    let agent_b = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_b, prefix, "grok-3", "key", &bucket, &xai_url,
    )
    .await
    .unwrap();
    agent_b
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("second"))],
        ))
        .await
        .unwrap();

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "expected two HTTP requests");
    assert_eq!(
        bodies[1]
            .get("previous_response_id")
            .and_then(|v| v.as_str()),
        Some("resp_kv_0"),
        "second turn must carry previous_response_id from KV: {:?}",
        bodies[1],
    );

    cleanup(&url, &bucket).await;
}

/// `prompt()` publishes `SessionNotification` messages to real NATS.
#[tokio::test]
async fn kv_prompt_publishes_session_notifications_to_nats() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_agent = real_nats(&url).await;
    let nats_sub = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();
    let xai_url = fake_xai_sse_kv("resp_kv_notif", &["Notification test"]).await;

    let agent = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_agent,
        prefix.clone(),
        "grok-3",
        "key",
        &bucket,
        &xai_url,
    )
    .await
    .unwrap();
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let notify_subject = format!("test.session.{sid}.client.session.update");
    let mut sub = nats_sub.subscribe(notify_subject).await.unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), sub.next())
        .await
        .expect("timed out waiting for session notification")
        .expect("subscription closed unexpectedly");

    let notif: agent_client_protocol::SessionNotification =
        serde_json::from_slice(&msg.payload).expect("must be valid SessionNotification JSON");
    assert_eq!(notif.session_id.to_string(), sid);

    cleanup(&url, &bucket).await;
}

/// Per-session user `api_key` (stored via `authenticate()` + `new_session()`)
/// survives an agent restart. A new agent instance opening the same KV bucket
/// must be able to call `prompt()` using only the key stored in the session —
/// without a global key and without re-authenticating.
#[tokio::test]
async fn kv_user_api_key_persists_across_agent_restart() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();
    let xai_url = fake_xai_sse_kv("resp_kv_apikey", &["reply after restart"]).await;

    // Agent A: no global key ("").
    let agent_a = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_a,
        prefix.clone(),
        "grok-3",
        "",
        &bucket,
        "http://localhost:1",
    )
    .await
    .unwrap();

    // Authenticate with a user key, then create a session — key is stored in KV.
    let mut meta = serde_json::Map::new();
    meta.insert("XAI_API_KEY".to_string(), serde_json::json!("user-key-kv"));
    agent_a
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(meta))
        .await
        .unwrap();
    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    // Agent B: also no global key. Opens the same KV bucket (simulates restart).
    let agent_b =
        XaiAgent::new_with_kv_bucket_and_xai_url(nats_b, prefix, "grok-3", "", &bucket, &xai_url)
            .await
            .unwrap();

    // Must be able to prompt without re-authenticating — uses key from KV.
    let resp = agent_b
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("hello after restart"))],
        ))
        .await;
    assert!(
        resp.is_ok(),
        "prompt must succeed with KV-stored api_key: {resp:?}"
    );

    cleanup(&url, &bucket).await;
}

/// `fork_session` stores `last_response_id: None` in KV even when the parent
/// session has a non-None last_response_id. The fork must start a fresh
/// stateful context — carrying the parent's response ID would cause the first
/// prompt on the fork to reference a stale/wrong response chain.
#[tokio::test]
async fn kv_fork_clears_last_response_id() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();
    let xai_url = fake_xai_sse_kv("resp_kv_parent", &["parent reply"]).await;

    let agent_a = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_a,
        prefix.clone(),
        "grok-3",
        "key",
        &bucket,
        &xai_url,
    )
    .await
    .unwrap();
    let sess = agent_a
        .new_session(NewSessionRequest::new("/src"))
        .await
        .unwrap();
    let parent_id = sess.session_id.to_string();

    // Prompt so the parent session accumulates a last_response_id in KV.
    agent_a
        .prompt(PromptRequest::new(
            parent_id.clone(),
            vec![ContentBlock::Text(TextContent::new("parent turn"))],
        ))
        .await
        .unwrap();
    let parent_resp_id = agent_a.test_session_last_response_id(&parent_id).await;
    assert_eq!(
        parent_resp_id.as_deref(),
        Some("resp_kv_parent"),
        "parent must have resp_id"
    );

    // Fork via agent A — fork data is written to KV.
    let fork = agent_a
        .fork_session(ForkSessionRequest::new(parent_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    // Agent B opens the same bucket and reads the fork from KV.
    let agent_b = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_b,
        prefix,
        "grok-3",
        "key",
        &bucket,
        "http://localhost:1",
    )
    .await
    .unwrap();

    let fork_resp_id = agent_b.test_session_last_response_id(&fork_id).await;
    assert!(
        fork_resp_id.is_none(),
        "fork must have last_response_id=None in KV, got: {fork_resp_id:?}",
    );

    cleanup(&url, &bucket).await;
}

/// `list_sessions` returns the correct `cwd` for each session when reading
/// from a real KV bucket. Verifies the cwd field round-trips through KV
/// serialization correctly.
#[tokio::test]
async fn kv_list_sessions_returns_correct_cwd() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent = XaiAgent::new_with_kv_bucket(nats_a, prefix, "grok-3", "key", &bucket)
        .await
        .unwrap();

    agent
        .new_session(NewSessionRequest::new("/project/alpha"))
        .await
        .unwrap();
    agent
        .new_session(NewSessionRequest::new("/project/beta"))
        .await
        .unwrap();
    agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();

    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert_eq!(list.sessions.len(), 3);

    let mut cwds: Vec<String> = list
        .sessions
        .iter()
        .map(|s| s.cwd.to_string_lossy().to_string())
        .collect();
    cwds.sort();
    assert_eq!(
        cwds,
        vec!["/project/alpha", "/project/beta", "/tmp"],
        "list_sessions must return correct cwds from KV: {cwds:?}",
    );

    cleanup(&url, &bucket).await;
}

// ── additional HTTP helpers ───────────────────────────────────────────────────

/// Single-connection recording server: captures the JSON request body and
/// responds with SSE chunks tagged with `resp_id`, then `[DONE]`.
async fn fake_xai_recording_one(
    resp_id: &'static str,
    chunks: Vec<&'static str>,
) -> (String, Arc<Mutex<Option<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body_slot: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let slot_clone = Arc::clone(&body_slot);

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let cl = drain_request_kv(&mut reader).await;
            let json = if cl > 0 {
                let mut buf = vec![0u8; cl];
                reader.read_exact(&mut buf).await.ok();
                serde_json::from_slice(&buf).unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            };
            *slot_clone.lock().unwrap() = Some(json);

            let sse: String = chunks.iter()
                .map(|t| format!("data: {{\"id\":\"{resp_id}\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"))
                .chain(std::iter::once("data: [DONE]\n\n".to_string()))
                .collect();
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), body_slot)
}

/// Multi-shot SSE server: accepts `responses.len()` connections sequentially,
/// each returning the corresponding `(resp_id, chunks)` as SSE.
async fn fake_xai_multi_sse(responses: Vec<(&'static str, Vec<&'static str>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        for (resp_id, chunks) in responses {
            if let Ok((stream, _)) = listener.accept().await {
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);
                drain_request_kv(&mut reader).await;
                let sse: String = chunks.iter()
                    .map(|t| format!("data: {{\"id\":\"{resp_id}\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"))
                    .chain(std::iter::once("data: [DONE]\n\n".to_string()))
                    .collect();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    sse.len(),
                    sse,
                );
                writer.write_all(http.as_bytes()).await.ok();
            }
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

// ── crash-recovery and history tests ─────────────────────────────────────────

/// When an agent crashes after writing the user message to KV but before
/// writing the assistant reply, the next agent that opens the same bucket
/// must detect the dangling user message and NOT duplicate it in the xAI
/// request (idempotency check). The xAI request must contain exactly one
/// user message.
#[tokio::test]
async fn kv_crash_recovery_idempotency_check() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    // Agent A creates a session then "crashes" — simulate by manually writing
    // the user message into KV without an assistant reply.
    let agent_a = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_a,
        prefix.clone(),
        "grok-3",
        "key",
        &bucket,
        "http://localhost:1",
    )
    .await
    .unwrap();
    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();
    // Inject the dangling user message directly into KV.
    agent_a
        .test_set_session_history(&sid, vec![Message::user("hello from kv")])
        .await;

    // Agent B opens the same bucket (simulates restart after crash).
    let (xai_url, body) = fake_xai_recording_one("resp_recovery", vec!["recovery reply"]).await;
    let agent_b = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_b, prefix, "grok-3", "key", &bucket, &xai_url,
    )
    .await
    .unwrap();

    // Prompt with the same user message — idempotency check must fire.
    let resp = agent_b
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("hello from kv"))],
        ))
        .await;
    assert!(
        resp.is_ok(),
        "prompt must succeed after crash recovery: {resp:?}"
    );

    // xAI must have received exactly ONE user message — no duplicate.
    let captured = body.lock().unwrap();
    let body_val = captured.as_ref().expect("xAI must have been called");
    let input = body_val["input"]
        .as_array()
        .expect("input must be an array");
    let user_msgs: Vec<_> = input.iter().filter(|m| m["role"] == "user").collect();
    assert_eq!(
        user_msgs.len(),
        1,
        "xAI request must contain exactly one user message (no duplicate): {input:?}"
    );

    // History: user + assistant (clean recovery, no duplicate).
    let history = agent_b.test_session_history(&sid).await;
    assert_eq!(
        history.len(),
        2,
        "history must be user+assistant after recovery: {history:?}"
    );
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");

    cleanup(&url, &bucket).await;
}

/// History trimming writes the trimmed slice to KV. A new agent instance
/// that opens the same bucket must load the already-trimmed history and
/// continue trimming correctly on subsequent turns.
#[tokio::test]
async fn kv_history_trimming_round_trips_through_kv() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();
    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    // 4-connection SSE server: 3 turns for agent_a, 1 turn for agent_b.
    let xai_url = fake_xai_multi_sse(vec![
        ("resp_1", vec!["reply1"]),
        ("resp_2", vec!["reply2"]),
        ("resp_3", vec!["reply3"]),
        ("resp_4", vec!["reply4"]),
    ])
    .await;

    // Agent A with max_history_messages=4 (keeps at most 2 turns).
    let agent_a = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_a,
        prefix.clone(),
        "grok-3",
        "key",
        &bucket,
        &xai_url,
    )
    .await
    .unwrap()
    .with_max_history_messages(4);

    let sess = agent_a
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    // Turn 1 → [user1, assistant1] — 2 msgs, under limit.
    agent_a
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("turn1"))],
        ))
        .await
        .unwrap();
    // Turn 2 → [user1, assistant1, user2, assistant2] — 4 msgs, at limit.
    agent_a
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("turn2"))],
        ))
        .await
        .unwrap();
    // Turn 3 → trim fires: [user2, assistant2, user3, assistant3] — 4 msgs in KV.
    agent_a
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("turn3"))],
        ))
        .await
        .unwrap();

    let history_a = agent_a.test_session_history(&sid).await;
    assert_eq!(
        history_a.len(),
        4,
        "agent_a must have 4 messages after trim: {history_a:?}"
    );
    assert_eq!(history_a[0].content_str(), "turn2");
    assert_eq!(history_a[2].content_str(), "turn3");

    // Agent B opens the same bucket — loads trimmed history from KV.
    let agent_b = XaiAgent::new_with_kv_bucket_and_xai_url(
        nats_b, prefix, "grok-3", "key", &bucket, &xai_url,
    )
    .await
    .unwrap()
    .with_max_history_messages(4);

    let history_b_before = agent_b.test_session_history(&sid).await;
    assert_eq!(
        history_b_before.len(),
        4,
        "agent_b must load 4 messages (trimmed) from KV: {history_b_before:?}"
    );

    // Turn 4 via agent_b → trim fires again: [user3, assistant3, user4, assistant4].
    agent_b
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("turn4"))],
        ))
        .await
        .unwrap();

    let history_b_after = agent_b.test_session_history(&sid).await;
    assert_eq!(
        history_b_after.len(),
        4,
        "after turn4 history must still be 4 (trimmed): {history_b_after:?}"
    );
    assert_eq!(history_b_after[0].content_str(), "turn3");
    assert_eq!(history_b_after[2].content_str(), "turn4");

    cleanup(&url, &bucket).await;
}
