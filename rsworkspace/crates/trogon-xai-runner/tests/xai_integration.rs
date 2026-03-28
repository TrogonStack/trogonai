//! Integration tests for `XaiAgent`.
//!
//! All tests hold `ENV_LOCK` to serialise env-var access (`XAI_BASE_URL`,
//! `XAI_MODELS`). `make_agent` resets both vars to a known state at the start
//! of each call, so no test sees a leftover value from a previous run.
//!
//! Run with: `cargo test -p trogon-xai-runner --features test-helpers`

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    Agent, AuthMethod, AuthenticateRequest, CancelNotification, CloseSessionRequest, ContentBlock,
    ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, ResumeSessionRequest, SetSessionModelRequest, StopReason,
    TextContent,
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
    // Reset ALL env vars to clean defaults so no test leaks state.
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

/// Like `make_agent` but also sets `XAI_MODELS`. Caller must hold `env_lock()`.
async fn make_agent_with_models(models_env: &str) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_BASE_URL");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::set_var("XAI_MODELS", models_env);
    }
    XaiAgent::new(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

/// Like `make_agent` but passes an empty api_key (no server-wide key).
/// Users must authenticate with their own key before sessions can prompt.
/// Caller must hold `env_lock()`.
async fn make_agent_no_key(base_url: Option<&str>) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "")
}

/// Like `make_agent` but sets `XAI_PROMPT_TIMEOUT_SECS`. Caller must hold `env_lock()`.
async fn make_agent_with_timeout(base_url: Option<&str>, timeout_secs: u64) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", timeout_secs.to_string());
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
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

/// Like `fake_xai_sse` but also captures the value of the `Authorization`
/// header sent by the agent. Returns (base_url, captured_header_value).
async fn fake_xai_sse_capturing_auth(text_chunks: &[&'static str]) -> (String, Arc<Mutex<Option<String>>>) {
    let body: String = text_chunks
        .iter()
        .map(|t| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n"))
        .chain(std::iter::once("data: [DONE]\n\n".to_string()))
        .collect();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let cap = Arc::clone(&captured);

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);

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
                // Preserve original casing of value but compare key case-insensitively.
                if trimmed.to_lowercase().starts_with("authorization:") {
                    let colon = trimmed.find(':').unwrap();
                    *cap.lock().unwrap() = Some(trimmed[colon + 1..].trim().to_string());
                }
                if let Some(rest) = trimmed.to_lowercase().strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap_or(0);
                }
            }
            if content_length > 0 {
                let mut buf = vec![0u8; content_length];
                reader.read_exact(&mut buf).await.ok();
            }

            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), captured)
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

// ── new fake server helpers ───────────────────────────────────────────────────

/// Sends a single SSE data line split across two TCP writes (simulates a
/// packet boundary inside an SSE line). The assembled content is "Hello".
/// No Content-Length — streaming response.
async fn fake_xai_sse_chunked() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;

            // Headers only — no Content-Length, so reqwest streams bytes as
            // they arrive rather than waiting for the body to be fully buffered.
            writer
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await
                .ok();

            // First write: the SSE line is cut mid-JSON (no trailing newline).
            writer
                .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel")
                .await
                .ok();
            writer.flush().await.ok();

            tokio::time::sleep(Duration::from_millis(20)).await;

            // Second write: completes the JSON, adds the line terminator, then DONE.
            writer.write_all(b"lo\"}}]}\n\ndata: [DONE]\n\n").await.ok();
            writer.flush().await.ok();
        }
    });

    format!("http://127.0.0.1:{port}/v1")
}

/// Recording server. Accepts `responses.len()` connections sequentially.
/// For each connection, reads and parses the request body JSON (stored in
/// the returned `Arc`), then responds with the Nth list of text chunks.
async fn fake_xai_sse_recording(
    responses: Vec<Vec<&'static str>>,
) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let bodies_clone = Arc::clone(&bodies);

    tokio::spawn(async move {
        for chunks in responses {
            if let Ok((stream, _)) = listener.accept().await {
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);

                // Read headers and capture Content-Length.
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

                // Read and parse the body.
                let body_json = if content_length > 0 {
                    let mut body = vec![0u8; content_length];
                    reader.read_exact(&mut body).await.ok();
                    serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                };
                bodies_clone.lock().unwrap().push(body_json);

                // Respond with SSE chunks.
                let body: String = chunks
                    .iter()
                    .map(|t| {
                        format!(
                            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n"
                        )
                    })
                    .chain(std::iter::once("data: [DONE]\n\n".to_string()))
                    .collect();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                writer.write_all(http.as_bytes()).await.ok();
            }
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), bodies)
}

/// Parallel SSE server: accepts `count` connections, spawning a handler task
/// per connection so they are served concurrently. All connections receive
/// the same `text_chunks` response. Returns the base URL.
async fn fake_xai_sse_multi(count: usize, text_chunks: Vec<&'static str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let chunks = Arc::new(text_chunks);

    tokio::spawn(async move {
        for _ in 0..count {
            if let Ok((stream, _)) = listener.accept().await {
                let chunks = Arc::clone(&chunks);
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = BufReader::new(reader);
                    drain_request(&mut reader).await;

                    let body: String = chunks
                        .iter()
                        .map(|t| {
                            format!(
                                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n"
                            )
                        })
                        .chain(std::iter::once("data: [DONE]\n\n".to_string()))
                        .collect();
                    let http = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    writer.write_all(http.as_bytes()).await.ok();
                });
            }
        }
    });

    format!("http://127.0.0.1:{port}/v1")
}

// ── partial SSE across chunks ─────────────────────────────────────────────────

#[tokio::test]
async fn partial_sse_lines_across_chunks_are_assembled_correctly() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_chunked().await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("hi"))];
    let resp = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    // The SSE line was split mid-JSON across two TCP writes; the parser must
    // buffer bytes until a newline and reassemble "Hello" correctly.
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content, "Hello");
}

// ── prompt timeout ────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_times_out_when_server_stops_responding() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_slow("partial").await;
    // 1-second timeout — well below the server's 30-second hold.
    let agent = make_agent_with_timeout(Some(&url), 1).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("hello"))];
    let start = std::time::Instant::now();
    let resp = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    // Should finish in roughly 1 second — not 30.
    assert!(elapsed < Duration::from_secs(5), "timed out too late: {elapsed:?}");
    assert!(elapsed >= Duration::from_millis(900), "timed out too early: {elapsed:?}");

    // The partial text ("partial") was received before the timeout, so history
    // must be updated.
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content, "partial");
}

// ── history as context ────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_includes_history_as_context_on_subsequent_turns() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![
        vec!["First reply"],
        vec!["Second reply"],
    ])
    .await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // Turn 1.
    let c1 = vec![ContentBlock::Text(TextContent::new("first question"))];
    agent.prompt(PromptRequest::new(session_id.clone(), c1)).await.unwrap();

    // Turn 2.
    let c2 = vec![ContentBlock::Text(TextContent::new("second question"))];
    agent.prompt(PromptRequest::new(session_id.clone(), c2)).await.unwrap();

    // Inspect what the agent sent on turn 2: it must include the full history.
    let captured = bodies.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected two recorded requests");
    let msgs = captured[1]["messages"]
        .as_array()
        .expect("turn-2 body must have a 'messages' array");

    // user(turn1) + assistant(turn1) + user(turn2) = 3 messages.
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "first question");
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"], "First reply");
    assert_eq!(msgs[2]["role"], "user");
    assert_eq!(msgs[2]["content"], "second question");
}

// ── concurrent session isolation ─────────────────────────────────────────────

#[tokio::test]
async fn concurrent_sessions_do_not_cross_contaminate() {
    let _guard = env_lock().lock().unwrap();
    // Two connections needed — one per concurrent prompt.
    let url = fake_xai_sse_multi(2, vec!["session-reply"]).await;
    let agent = make_agent(Some(&url)).await;

    let sess_a = agent.new_session(NewSessionRequest::new("/tmp/a")).await.unwrap();
    let sess_b = agent.new_session(NewSessionRequest::new("/tmp/b")).await.unwrap();
    let id_a = sess_a.session_id.to_string();
    let id_b = sess_b.session_id.to_string();

    let c_a = vec![ContentBlock::Text(TextContent::new("question-a"))];
    let c_b = vec![ContentBlock::Text(TextContent::new("question-b"))];

    // XaiAgent is !Send, so use tokio::join! (runs on one thread) instead of
    // tokio::spawn (which requires Send).
    let (r_a, r_b) = tokio::join!(
        agent.prompt(PromptRequest::new(id_a.clone(), c_a)),
        agent.prompt(PromptRequest::new(id_b.clone(), c_b)),
    );
    assert_eq!(r_a.unwrap().stop_reason, StopReason::EndTurn);
    assert_eq!(r_b.unwrap().stop_reason, StopReason::EndTurn);

    // Each session must hold only its own exchange — no cross-contamination.
    let hist_a = agent.test_session_history(&id_a).await;
    let hist_b = agent.test_session_history(&id_b).await;

    assert_eq!(hist_a.len(), 2);
    assert_eq!(hist_a[0].content, "question-a");
    assert_eq!(hist_a[1].content, "session-reply");

    assert_eq!(hist_b.len(), 2);
    assert_eq!(hist_b[0].content, "question-b");
    assert_eq!(hist_b[1].content, "session-reply");
}

// ── authenticate ──────────────────────────────────────────────────────────────

/// Build a meta map with a single XAI_API_KEY entry (test helper).
fn api_key_meta(key: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("XAI_API_KEY".to_string(), serde_json::json!(key));
    m
}

#[tokio::test]
async fn initialize_always_advertises_xai_api_key_method() {
    let _guard = env_lock().lock().unwrap();
    // Agent with server key — should still expose the per-user method.
    let agent = make_agent(None).await;
    let resp = agent.initialize(InitializeRequest::new(agent_client_protocol::ProtocolVersion::LATEST)).await.unwrap();
    assert!(
        resp.auth_methods.iter().any(|m| m.id().to_string() == "xai-api-key"),
        "expected 'xai-api-key' in auth_methods",
    );
}

#[tokio::test]
async fn initialize_advertises_agent_method_only_when_server_key_configured() {
    let _guard = env_lock().lock().unwrap();

    // With server key: both methods present.
    let with_key = make_agent(None).await;
    let resp = with_key.initialize(InitializeRequest::new(agent_client_protocol::ProtocolVersion::LATEST)).await.unwrap();
    assert!(
        resp.auth_methods.iter().any(|m| m.id().to_string() == "agent"),
        "expected 'agent' method when server key is set",
    );

    // Without server key: only 'xai-api-key' present.
    let no_key = make_agent_no_key(None).await;
    let resp = no_key.initialize(InitializeRequest::new(agent_client_protocol::ProtocolVersion::LATEST)).await.unwrap();
    assert!(
        !resp.auth_methods.iter().any(|m| m.id().to_string() == "agent"),
        "should not advertise 'agent' method without server key",
    );
}

#[tokio::test]
async fn authenticate_with_user_key_enables_prompt() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse(&["response"]).await;
    let agent = make_agent_no_key(Some(&url)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("user-test-key")))
        .await
        .unwrap();

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn authenticate_agent_method_uses_server_key() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse(&["response"]).await;
    let agent = make_agent(Some(&url)).await; // has "fake-key" as server key

    agent
        .authenticate(AuthenticateRequest::new("agent"))
        .await
        .unwrap();

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn authenticate_missing_key_in_meta_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(None).await;

    // No meta at all.
    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key"))
        .await
        .unwrap_err();
    assert!(err.message.contains("XAI_API_KEY missing"), "got: {}", err.message);

    // Meta present but wrong key name.
    let mut bad_meta = serde_json::Map::new();
    bad_meta.insert("WRONG_KEY".to_string(), serde_json::json!("value"));
    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(bad_meta))
        .await
        .unwrap_err();
    assert!(err.message.contains("XAI_API_KEY missing"), "got: {}", err.message);
}

#[tokio::test]
async fn authenticate_agent_method_without_server_key_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(None).await;

    let err = agent
        .authenticate(AuthenticateRequest::new("agent"))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("no server API key configured"),
        "got: {}",
        err.message,
    );
}

#[tokio::test]
async fn authenticate_unknown_method_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let err = agent
        .authenticate(AuthenticateRequest::new("oauth2-mystery"))
        .await
        .unwrap_err();
    assert!(err.message.contains("unknown method"), "got: {}", err.message);
}

#[tokio::test]
async fn prompt_fails_with_clear_error_when_session_has_no_api_key() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(None).await;

    // Create session without authenticating.
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let err = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("no API key"),
        "expected 'no API key' in error, got: {}",
        err.message,
    );
}

#[tokio::test]
async fn forked_session_inherits_api_key_from_parent() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_multi(2, vec!["reply"]).await;
    let agent = make_agent_no_key(Some(&url)).await;

    // Authenticate and create parent.
    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("user-key")))
        .await
        .unwrap();
    let parent = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let parent_id = parent.session_id.to_string();

    // Fork — no re-authentication needed; child inherits the parent's key.
    let fork = agent
        .fork_session(ForkSessionRequest::new(parent_id.clone(), "/tmp/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    let (r_parent, r_fork) = tokio::join!(
        agent.prompt(PromptRequest::new(
            parent_id,
            vec![ContentBlock::Text(TextContent::new("from parent"))],
        )),
        agent.prompt(PromptRequest::new(
            fork_id,
            vec![ContentBlock::Text(TextContent::new("from fork"))],
        )),
    );
    assert_eq!(r_parent.unwrap().stop_reason, StopReason::EndTurn);
    assert_eq!(r_fork.unwrap().stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn authenticate_user_key_is_sent_as_bearer_token_to_xai() {
    let _guard = env_lock().lock().unwrap();
    let (url, captured_auth) = fake_xai_sse_capturing_auth(&["reply"]).await;
    let agent = make_agent_no_key(Some(&url)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("my-user-key-123")))
        .await
        .unwrap();
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let auth = captured_auth.lock().unwrap().clone()
        .expect("Authorization header was not captured");
    assert_eq!(auth, "Bearer my-user-key-123");
}

#[tokio::test]
async fn pending_api_key_consumed_by_first_new_session() {
    let _guard = env_lock().lock().unwrap();
    // Agent with no global key — only the pending key from authenticate.
    let url = fake_xai_sse(&["reply"]).await;
    let agent = make_agent_no_key(Some(&url)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("user-key")))
        .await
        .unwrap();

    let sess1 = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sess2 = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();

    // sess2 was created after the pending key was consumed — no key, no global fallback.
    let err = agent
        .prompt(PromptRequest::new(
            sess2.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("no API key"),
        "expected 'no API key' for keyless session, got: {}",
        err.message,
    );

    // sess1 still has the user key and succeeds (one fake HTTP connection used here).
    let resp = agent
        .prompt(PromptRequest::new(
            sess1.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn initialize_env_var_method_advertises_correct_var_name_and_link() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let resp = agent
        .initialize(InitializeRequest::new(agent_client_protocol::ProtocolVersion::LATEST))
        .await
        .unwrap();

    let ev = resp
        .auth_methods
        .iter()
        .find_map(|m| match m {
            AuthMethod::EnvVar(ev) => Some(ev),
            _ => None,
        })
        .expect("expected an EnvVar auth method in initialize response");

    // Client UIs read `vars` to know which field to prompt for.
    assert_eq!(ev.vars.len(), 1);
    assert_eq!(ev.vars[0].name, "XAI_API_KEY");
    // `link` tells the UI where the user can obtain a key.
    assert_eq!(ev.link.as_deref(), Some("https://x.ai/api"));
}

// ── session usable after cancel ───────────────────────────────────────────────

#[tokio::test]
async fn session_is_usable_after_cancel() {
    let _guard = env_lock().lock().unwrap();

    // Two-connection server: first holds (cancelable), second responds normally.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");

    tokio::spawn(async move {
        // Connection 1 — slow, for the canceled prompt.
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            writer.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n").await.ok();
            writer.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n").await.ok();
            writer.flush().await.ok();
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        // Connection 2 — normal, for the second prompt.
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"second reply\"}}]}\n\ndata: [DONE]\n\n";
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(), body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });

    let agent = std::sync::Arc::new(make_agent(Some(&url)).await);
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // Prompt 1: cancel it mid-stream.
    let agent2 = std::sync::Arc::clone(&agent);
    let sid = session_id.clone();
    let (r1, _) = tokio::join!(
        agent.prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("first"))],
        )),
        async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            agent2.cancel(CancelNotification::new(sid)).await.unwrap();
        }
    );
    assert_eq!(r1.unwrap().stop_reason, StopReason::EndTurn);
    assert!(agent.test_session_history(&session_id).await.is_empty(), "canceled turn must not update history");

    // Prompt 2: session must still accept new prompts.
    let r2 = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("second"))],
        ))
        .await
        .unwrap();
    assert_eq!(r2.stop_reason, StopReason::EndTurn);

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "second");
    assert_eq!(history[1].content, "second reply");
}

// ── re-authentication replaces pending key ────────────────────────────────────

#[tokio::test]
async fn re_authentication_replaces_pending_key() {
    let _guard = env_lock().lock().unwrap();
    let (url, captured_auth) = fake_xai_sse_capturing_auth(&["reply"]).await;
    let agent = make_agent_no_key(Some(&url)).await;

    // Authenticate twice — second key must win.
    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("key-first")))
        .await
        .unwrap();
    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("key-second")))
        .await
        .unwrap();

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let auth = captured_auth.lock().unwrap().clone().expect("Authorization header not captured");
    assert_eq!(auth, "Bearer key-second");
}

// ── multi-block prompt content ────────────────────────────────────────────────

#[tokio::test]
async fn prompt_multi_block_content_is_joined_with_newline() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let content = vec![
        ContentBlock::Text(TextContent::new("line one")),
        ContentBlock::Text(TextContent::new("line two")),
    ];
    agent
        .prompt(PromptRequest::new(sess.session_id.to_string(), content))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let msgs = captured[0]["messages"].as_array().expect("messages array");
    let user_msg = msgs.last().expect("at least one message");
    assert_eq!(user_msg["role"], "user");
    assert_eq!(user_msg["content"], "line one\nline two");
}

// ── set_session_model affects HTTP request ────────────────────────────────────

#[tokio::test]
async fn set_session_model_is_used_in_http_request_to_xai() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            session_id,
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    assert_eq!(captured[0]["model"], "grok-3-mini");
}

// ── load_session reflects updated model ──────────────────────────────────────

#[tokio::test]
async fn load_session_reflects_model_after_set_session_model() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap();

    let loaded = agent
        .load_session(LoadSessionRequest::new(session_id, "/tmp"))
        .await
        .unwrap();
    let models = loaded.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3-mini");
}

// ── fork history independence ─────────────────────────────────────────────────

#[tokio::test]
async fn fork_histories_are_independent_after_diverging_prompts() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_multi(3, vec!["reply"]).await;
    let agent = make_agent(Some(&url)).await;

    // Create parent and prompt once to build shared history.
    let src = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let src_id = src.session_id.to_string();
    agent
        .prompt(PromptRequest::new(
            src_id.clone(),
            vec![ContentBlock::Text(TextContent::new("shared"))],
        ))
        .await
        .unwrap();

    // Fork inherits the one-exchange history.
    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    // Prompt each independently.
    let (r_src, r_fork) = tokio::join!(
        agent.prompt(PromptRequest::new(
            src_id.clone(),
            vec![ContentBlock::Text(TextContent::new("parent-only"))],
        )),
        agent.prompt(PromptRequest::new(
            fork_id.clone(),
            vec![ContentBlock::Text(TextContent::new("fork-only"))],
        )),
    );
    r_src.unwrap();
    r_fork.unwrap();

    let src_history = agent.test_session_history(&src_id).await;
    let fork_history = agent.test_session_history(&fork_id).await;

    // Each has: shared exchange + its own exchange = 4 messages.
    assert_eq!(src_history.len(), 4);
    assert_eq!(fork_history.len(), 4);

    // The diverging turns must be independent.
    assert_eq!(src_history[2].content, "parent-only");
    assert_eq!(fork_history[2].content, "fork-only");

    // The other session must not contain the diverging turn of its sibling.
    assert!(src_history.iter().all(|m| m.content != "fork-only"));
    assert!(fork_history.iter().all(|m| m.content != "parent-only"));
}
