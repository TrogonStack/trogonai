//! Integration tests for `XaiAgent`.
//!
//! All tests hold `ENV_LOCK` to serialise env-var access (`XAI_BASE_URL`,
//! `XAI_MODELS`). `make_agent` resets both vars to a known state at the start
//! of each call, so no test sees a leftover value from a previous run.
//!
//! Run with: `cargo test -p trogon-xai-runner --features test-helpers`

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    Agent, CancelNotification, CloseSessionRequest, ContentBlock, ForkSessionRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SetSessionModelRequest, StopReason, TextContent,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use trogon_xai_runner::XaiAgent;

// ── env-var lock ──────────────────────────────────────────────────────────────

/// All tests must hold this lock for the duration of env-var writes + agent
/// construction. `make_agent` resets env vars at entry, so tests that set
/// custom values do so BEFORE calling `make_agent`.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ── fake NATS ─────────────────────────────────────────────────────────────────

/// Minimal fake NATS: handles INFO/CONNECT/PING handshake only.
/// `session_notification` in `XaiAgent::prompt` is fire-and-forget, so PUB
/// commands are silently ignored.
async fn fake_nats() -> async_nats::Client {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            writer
                .write_all(
                    b"INFO {\"server_id\":\"test\",\"version\":\"2.10.0\",\
                      \"max_payload\":1048576,\"proto\":1,\"headers\":true}\r\n",
                )
                .await
                .ok();
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("CONNECT") {
                    writer.write_all(b"+OK\r\n").await.ok();
                } else if line.starts_with("PING") {
                    writer.write_all(b"PONG\r\n").await.ok();
                }
            }
        }
    });

    async_nats::connect(format!("nats://127.0.0.1:{port}")).await.unwrap()
}

// ── agent factory ─────────────────────────────────────────────────────────────

/// Build an `XaiAgent`. Resets `XAI_BASE_URL` and `XAI_MODELS` to a clean
/// state at entry, then applies `base_url` if given.
///
/// Callers that need custom `XAI_MODELS` must set the env var BEFORE calling
/// this function (while still holding `env_lock()`).
///
/// Caller must hold `env_lock()`.
async fn make_agent(base_url: Option<&str>) -> XaiAgent {
    // Reset to clean defaults so no test leaks state into the next.
    unsafe { std::env::remove_var("XAI_MODELS") };
    match base_url {
        Some(url) => unsafe { std::env::set_var("XAI_BASE_URL", url) },
        None => unsafe { std::env::remove_var("XAI_BASE_URL") },
    }
    XaiAgent::new(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

/// Like `make_agent` but also sets `XAI_MODELS` before creating the agent.
/// Caller must hold `env_lock()`.
async fn make_agent_with_models(models_env: &str) -> XaiAgent {
    unsafe { std::env::remove_var("XAI_BASE_URL") };
    unsafe { std::env::set_var("XAI_MODELS", models_env) };
    XaiAgent::new(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

// ── fake HTTP servers ─────────────────────────────────────────────────────────

/// Shared request-drain logic: reads past HTTP headers and body.
async fn drain_request(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) {
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
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await.ok();
    }
}

/// One-shot SSE server: each `text_chunks` string becomes a `data:` event,
/// followed by `[DONE]`. Returns the base URL.
async fn fake_xai_sse(text_chunks: &[&'static str]) -> String {
    let body: String = text_chunks
        .iter()
        .map(|t| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n"))
        .chain(std::iter::once("data: [DONE]\n\n".to_string()))
        .collect();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;

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

/// Slow SSE server: sends one text chunk, then holds the connection open
/// indefinitely (no `[DONE]`). Used to exercise cancellation.
async fn fake_xai_sse_slow(first_chunk: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;

            // Send headers without Content-Length: connection stays open.
            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
            writer.write_all(header.as_bytes()).await.ok();
            let chunk = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{first_chunk}\"}}}}]}}\n\n"
            );
            writer.write_all(chunk.as_bytes()).await.ok();
            writer.flush().await.ok();

            // Hold the connection open until the client drops it.
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    format!("http://127.0.0.1:{port}/v1")
}

/// Error server: returns an HTTP error response with no body chunks.
async fn fake_xai_error(status: u16) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;

            let error_body = r#"{"error":{"message":"API error"}}"#;
            let http = format!(
                "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                error_body.len(),
                error_body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });

    format!("http://127.0.0.1:{port}/v1")
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_creates_session_with_correct_model_state() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let resp = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();

    assert!(!resp.session_id.to_string().is_empty());
    let models = resp.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3");

    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert_eq!(list.sessions.len(), 1);
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_is_empty_before_any_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert!(list.sessions.is_empty());
}

#[tokio::test]
async fn list_sessions_sorted_by_session_id() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    agent.new_session(NewSessionRequest::new("/a")).await.unwrap();
    agent.new_session(NewSessionRequest::new("/b")).await.unwrap();
    agent.new_session(NewSessionRequest::new("/c")).await.unwrap();

    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert_eq!(list.sessions.len(), 3);

    let ids: Vec<String> = list.sessions.iter().map(|s| s.session_id.to_string()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "sessions must be sorted by id");
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_returns_error_for_unknown_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let err = agent
        .load_session(LoadSessionRequest::new("no-such-id", "/tmp"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"), "error: {}", err.message);
}

#[tokio::test]
async fn load_session_returns_correct_state_for_existing_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let new = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = new.session_id.to_string();

    let loaded = agent
        .load_session(LoadSessionRequest::new(session_id.clone(), "/tmp"))
        .await
        .unwrap();
    let models = loaded.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3");
}

// ── close_session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn close_session_removes_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let new = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = new.session_id.to_string();

    agent.close_session(CloseSessionRequest::new(session_id.clone())).await.unwrap();

    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert!(list.sessions.is_empty());
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test]
async fn resume_session_returns_error_for_unknown_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let err = agent
        .resume_session(ResumeSessionRequest::new("no-such-id", "/tmp"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn resume_session_succeeds_for_existing_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let new = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = new.session_id.to_string();
    agent
        .resume_session(ResumeSessionRequest::new(session_id.clone(), "/tmp"))
        .await
        .unwrap();
}

// ── set_session_model ─────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_valid_model_persists() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let new = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = new.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap();

    let loaded = agent
        .load_session(LoadSessionRequest::new(session_id.clone(), "/tmp"))
        .await
        .unwrap();
    let models = loaded.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3-mini");
}

#[tokio::test]
async fn set_session_model_unknown_model_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let new = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = new.session_id.to_string();

    let err = agent
        .set_session_model(SetSessionModelRequest::new(session_id, "not-a-real-model"))
        .await
        .unwrap_err();
    assert!(err.message.contains("unknown model"));
}

#[tokio::test]
async fn set_session_model_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let err = agent
        .set_session_model(SetSessionModelRequest::new("no-such-id", "grok-3"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

// ── fork_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_creates_independent_session_inheriting_model() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let src = agent.new_session(NewSessionRequest::new("/src")).await.unwrap();
    let src_id = src.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(src_id.clone(), "grok-3-mini"))
        .await
        .unwrap();

    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    assert_ne!(fork_id, src_id);
    let models = fork.models.expect("fork should carry model state");
    assert_eq!(models.current_model_id.to_string(), "grok-3-mini");

    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert_eq!(list.sessions.len(), 2);
}

#[tokio::test]
async fn fork_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let err = agent
        .fork_session(ForkSessionRequest::new("no-such-id", "/fork"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn fork_session_clones_conversation_history() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse(&["Hi"]).await;
    let agent = make_agent(Some(&url)).await;

    let src = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let src_id = src.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("hello"))];
    agent.prompt(PromptRequest::new(src_id.clone(), content)).await.unwrap();

    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    assert_ne!(fork_id, src_id);
    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert_eq!(list.sessions.len(), 2);

    let src_history = agent.test_session_history(&src_id).await;
    let fork_history = agent.test_session_history(&fork_id).await;
    assert_eq!(src_history.len(), 2, "source: user + assistant");
    assert_eq!(fork_history.len(), 2, "fork should inherit history");
    assert_eq!(fork_history[0].content, src_history[0].content);
}

// ── prompt ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let content = vec![ContentBlock::Text(TextContent::new("hi"))];
    let err = agent
        .prompt(PromptRequest::new("no-such-id", content))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn prompt_returns_end_turn_and_updates_history() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse(&["Hello", " world"]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("ping"))];
    let resp = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2, "user + assistant messages");
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content, "ping");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content, "Hello world");
}

#[tokio::test]
async fn prompt_sends_accumulated_history_on_second_turn() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse(&["First reply"]).await;
    let agent = make_agent(Some(&url)).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let c1 = vec![ContentBlock::Text(TextContent::new("first turn"))];
    agent.prompt(PromptRequest::new(session_id.clone(), c1)).await.unwrap();

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "first turn");
    assert_eq!(history[1].content, "First reply");
}

#[tokio::test]
async fn prompt_with_api_error_returns_end_turn_without_updating_history() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_error(401).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("ping"))];
    let resp = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    // History must remain empty — no assistant text was produced.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "history should not be updated on API error");
}

#[tokio::test]
async fn prompt_with_done_only_response_does_not_update_history() {
    let _guard = env_lock().lock().unwrap();
    // Server sends [DONE] immediately with no text chunks.
    let url = fake_xai_sse(&[]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("ping"))];
    agent.prompt(PromptRequest::new(session_id.clone(), content)).await.unwrap();

    // No assistant text → history stays empty.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "history should not be updated when assistant produces no text");
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_unknown_session_is_a_noop() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    agent.cancel(CancelNotification::new("no-such-id")).await.unwrap();
}

#[tokio::test]
async fn cancel_interrupts_in_flight_prompt() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_slow("partial").await;
    let agent = std::sync::Arc::new(make_agent(Some(&url)).await);

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let agent_cancel = std::sync::Arc::clone(&agent);
    let cancel_session_id = session_id.clone();

    // Run prompt and cancel concurrently on the same task using join!.
    // Both futures share &agent (via Arc), which is fine since all methods
    // take &self. The prompt future is !Send (contains boxed_local stream),
    // so we use join! rather than spawn.
    let content = vec![ContentBlock::Text(TextContent::new("test"))];
    let (prompt_result, _) = tokio::join!(
        agent.prompt(PromptRequest::new(session_id.clone(), content)),
        async move {
            // Give the prompt time to register the cancel channel and start
            // streaming, then fire cancel.
            tokio::time::sleep(Duration::from_millis(20)).await;
            agent_cancel
                .cancel(CancelNotification::new(cancel_session_id))
                .await
                .unwrap();
        }
    );

    let resp = prompt_result.unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    // History must not be updated for a canceled turn.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "canceled prompt must not update history");
}

// ── XAI_MODELS env var ────────────────────────────────────────────────────────

#[tokio::test]
async fn xai_models_env_var_custom_models_accepted() {
    let _guard = env_lock().lock().unwrap();
    // Set BEFORE calling make_agent (make_agent reads it, then removes it internally).
    // We use make_agent_with_models which sets the var correctly.
    let agent = make_agent_with_models("model-a:Model A,model-b:Model B").await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // "grok-3" is the default model and gets auto-added even if not listed.
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "model-a"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "model-b"))
        .await
        .unwrap();
    // grok-3 auto-added
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .unwrap();
    // grok-3-mini NOT in the list
    let err = agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap_err();
    assert!(err.message.contains("unknown model"));
}

#[tokio::test]
async fn xai_models_env_var_malformed_entries_are_skipped() {
    let _guard = env_lock().lock().unwrap();
    // "malformed" has no colon — should be silently skipped.
    let agent = make_agent_with_models("valid:Valid,malformed,another:Another").await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "valid"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "another"))
        .await
        .unwrap();
    // "malformed" was skipped — not in the list
    let err = agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "malformed"))
        .await
        .unwrap_err();
    assert!(err.message.contains("unknown model"));
}

#[tokio::test]
async fn xai_models_default_when_env_var_not_set() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await; // make_agent removes XAI_MODELS
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // Default list includes grok-3 and grok-3-mini.
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap();
}

#[tokio::test]
async fn xai_models_default_model_auto_added_when_absent_from_list() {
    let _guard = env_lock().lock().unwrap();
    // Neither entry is "grok-3" — the default model. It should be auto-added.
    let agent = make_agent_with_models("alpha:Alpha,beta:Beta").await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // current model is still grok-3 (passed to new()) and was auto-added
    let models = sess.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3");

    // grok-3 is selectable even though it wasn't in XAI_MODELS
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .expect("auto-added default model must be selectable");
}
