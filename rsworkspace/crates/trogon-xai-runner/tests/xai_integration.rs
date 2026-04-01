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
    EmbeddedResource, EmbeddedResourceResource, ForkSessionRequest, InitializeRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, ProtocolVersion,
    ResumeSessionRequest, ResourceLink, SetSessionConfigOptionRequest, SetSessionModelRequest,
    StopReason, TextContent, TextResourceContents,
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
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

/// Like `make_agent` but also sets `XAI_MODELS`. Caller must hold `env_lock()`.
async fn make_agent_with_models(models_env: &str) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_BASE_URL");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::set_var("XAI_MODELS", models_env);
    }
    XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

/// Like `make_agent` but passes an empty api_key (no server-wide key).
/// Users must authenticate with their own key before sessions can prompt.
/// Caller must hold `env_lock()`.
async fn make_agent_no_key(base_url: Option<&str>) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "")
}

/// Like `make_agent` but sets `XAI_PROMPT_TIMEOUT_SECS`. Caller must hold `env_lock()`.
async fn make_agent_with_timeout(base_url: Option<&str>, timeout_secs: u64) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", timeout_secs.to_string());
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

/// Like `make_agent` but also sets `XAI_SYSTEM_PROMPT`. Caller must hold `env_lock()`.
async fn make_agent_with_system_prompt(base_url: Option<&str>, system_prompt: &str) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::set_var("XAI_SYSTEM_PROMPT", system_prompt);
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
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

/// One-shot SSE server: each `text_chunks` string becomes a Responses API
/// `message.delta` event, followed by `[DONE]`. Returns the base URL.
async fn fake_xai_sse(text_chunks: &[&'static str]) -> String {
    let body: String = text_chunks
        .iter()
        .map(|t| format!("data: {{\"id\":\"resp_test\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"))
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
                "data: {{\"id\":\"resp_partial\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{first_chunk}\"}}}}\n\n"
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
        .map(|t| format!("data: {{\"id\":\"resp_test\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"))
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
    assert_eq!(fork_history[0].content_str(), src_history[0].content_str());
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
    assert_eq!(history[0].content_str(), "ping");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content_str(), "Hello world");
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
    assert_eq!(history[0].content_str(), "first turn");
    assert_eq!(history[1].content_str(), "First reply");
}

#[tokio::test]
async fn prompt_with_api_error_returns_acp_error() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_error(401).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("ping"))];
    let err = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap_err();

    // Must propagate as an ACP error, not silently return EndTurn.
    assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError.into());

    // The user message is compensated away on xAI error — same as cancel.
    // The turn failed with no response; the retry should start from a clean state.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "history should be empty after xAI error: {history:?}");
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

    // The user message is durably persisted before the xAI call. No assistant
    // text was produced, so only the user turn survives in history.
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "ping");
}

// Retry after a failed put(assistant): history already ends with the user
// message, so the retry must not duplicate it in the xAI context.
#[tokio::test]
async fn prompt_retry_after_incomplete_turn_does_not_duplicate_user_message() {
    let _guard = env_lock().lock().unwrap();

    // One connection: the retry is the only prompt call.
    let (url, bodies) = fake_xai_sse_recording(vec![
        vec!["the reply"],
    ]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // Simulate a crash after put(user) but before put(assistant): manually
    // plant the user message in history without an assistant reply.
    {
        let mut data = agent.test_session_history(&session_id).await;
        // history is empty; push the user message as if it was persisted before
        // a crash that lost the assistant reply.
        data.push(trogon_xai_runner::Message::user("hello"));
        agent.test_set_session_history(&session_id, data).await;
    }

    // Retry with the same message. Should be treated as a resume, not a fresh turn.
    let resp = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    // History should have exactly one user + one assistant — no duplicate user.
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2, "expected user + assistant, got: {history:?}");
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "hello");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content_str(), "the reply");

    // The xAI request body must contain exactly one user message — no duplicate.
    let bodies = bodies.lock().unwrap();
    let input = bodies[0]["input"].as_array().unwrap();
    let user_msgs: Vec<_> = input.iter().filter(|m| m["role"] == "user").collect();
    assert_eq!(user_msgs.len(), 1, "xAI request must have exactly one user message: {input:?}");
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
    assert_eq!(resp.stop_reason, StopReason::Cancelled);

    // History must not be updated for a canceled turn.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "canceled prompt must not update history");
}

// cancel_channels entry is removed after prompt completes (no memory leak when
// sessions expire via TTL without an explicit close_session).
#[tokio::test]
async fn cancel_channels_entry_is_cleaned_up_after_prompt() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse(&["hello"]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    assert_eq!(agent.test_cancel_channels_len().await, 0);

    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hi"))],
        ))
        .await
        .unwrap();

    // Entry must be removed after prompt completes — no leak for TTL-expired sessions.
    assert_eq!(
        agent.test_cancel_channels_len().await,
        0,
        "cancel_channels must be empty after prompt completes"
    );
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
                .write_all(b"data: {\"id\":\"resp_test\",\"type\":\"message.delta\",\"delta\":{\"type\":\"output_text\",\"text\":\"Hel")
                .await
                .ok();
            writer.flush().await.ok();

            tokio::time::sleep(Duration::from_millis(20)).await;

            // Second write: completes the JSON, adds the line terminator, then DONE.
            writer.write_all(b"lo\"}}\n\ndata: [DONE]\n\n").await.ok();
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
                // Capture the index before pushing so resp_id matches the index
                // in the bodies array (resp_0 → bodies[0], resp_1 → bodies[1], …).
                let resp_idx = bodies_clone.lock().unwrap().len();
                bodies_clone.lock().unwrap().push(body_json);

                // Respond with SSE chunks (Responses API format).
                let resp_id = format!("resp_{resp_idx}");
                let body: String = chunks
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
                                "data: {{\"id\":\"resp_test\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"
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
    assert_eq!(history[1].content_str(), "Hello");
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

    assert_eq!(resp.stop_reason, StopReason::Cancelled);
    // Should finish in roughly 1 second — not 30.
    assert!(elapsed < Duration::from_secs(5), "timed out too late: {elapsed:?}");
    assert!(elapsed >= Duration::from_millis(900), "timed out too early: {elapsed:?}");

    // The partial text ("partial") was received before the timeout, so history
    // must be updated (unlike an explicit cancel, a timeout preserves partial text).
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content_str(), "partial");
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

    let captured = bodies.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected two recorded requests");

    // Turn 1: full history (just the user message — no prior context).
    let t1_input = captured[0]["input"].as_array()
        .expect("turn-1 body must have an 'input' array");
    assert_eq!(t1_input.len(), 1, "turn 1 should have exactly one input item");
    assert_eq!(t1_input[0]["role"], "user");
    assert_eq!(t1_input[0]["content"], "first question");
    assert!(captured[0]["previous_response_id"].is_null(), "turn 1 must not send previous_response_id");

    // Turn 2: stateful via previous_response_id — only the new user message.
    let t2_input = captured[1]["input"].as_array()
        .expect("turn-2 body must have an 'input' array");
    assert_eq!(t2_input.len(), 1, "turn 2 should send only the new user message (context held server-side)");
    assert_eq!(t2_input[0]["role"], "user");
    assert_eq!(t2_input[0]["content"], "second question");
    // The agent must reference the turn-1 response id so the server knows the context.
    assert_eq!(captured[1]["previous_response_id"], "resp_0",
        "turn 2 must reference turn 1's response id");
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
    assert_eq!(hist_a[0].content_str(), "question-a");
    assert_eq!(hist_a[1].content_str(), "session-reply");

    assert_eq!(hist_b.len(), 2);
    assert_eq!(hist_b[0].content_str(), "question-b");
    assert_eq!(hist_b[1].content_str(), "session-reply");
}

// ── authenticate ──────────────────────────────────────────────────────────────

/// Build a meta map with a single XAI_API_KEY entry (test helper).
fn api_key_meta(key: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("XAI_API_KEY".to_string(), serde_json::json!(key));
    m
}

#[tokio::test]
async fn initialize_declares_embedded_context_prompt_capability() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let resp = agent
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();
    assert!(
        resp.agent_capabilities.prompt_capabilities.embedded_context,
        "agent must declare embedded_context=true so clients send ContentBlock::Resource"
    );
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
async fn authenticate_empty_key_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(None).await;

    let mut meta = serde_json::Map::new();
    meta.insert("XAI_API_KEY".to_string(), serde_json::json!(""));
    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(meta))
        .await
        .unwrap_err();
    assert!(err.message.contains("must not be empty"), "got: {}", err.message);
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
            writer.write_all(b"data: {\"id\":\"resp_1\",\"type\":\"message.delta\",\"delta\":{\"type\":\"output_text\",\"text\":\"partial\"}}\n\n").await.ok();
            writer.flush().await.ok();
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        // Connection 2 — normal, for the second prompt.
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            let body = "data: {\"id\":\"resp_2\",\"type\":\"message.delta\",\"delta\":{\"type\":\"output_text\",\"text\":\"second reply\"}}\n\ndata: [DONE]\n\n";
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
    assert_eq!(r1.unwrap().stop_reason, StopReason::Cancelled);
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
    assert_eq!(history[0].content_str(), "second");
    assert_eq!(history[1].content_str(), "second reply");
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
    let input = captured[0]["input"].as_array().expect("input array");
    let user_msg = input.last().expect("at least one input item");
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
    assert_eq!(src_history[2].content_str(), "parent-only");
    assert_eq!(fork_history[2].content_str(), "fork-only");

    // The other session must not contain the diverging turn of its sibling.
    assert!(src_history.iter().all(|m| m.content_str() != "fork-only"));
    assert!(fork_history.iter().all(|m| m.content_str() != "parent-only"));
}

// ── edge-case tests ───────────────────────────────────────────────────────────

#[tokio::test]
async fn xai_prompt_timeout_secs_invalid_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("XAI_BASE_URL");
        std::env::remove_var("XAI_MODELS");
        std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "not-a-number");
    }
    let agent =
        XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key");
    assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(300));
}

#[tokio::test]
async fn authenticate_non_string_key_in_meta_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(None).await;

    let mut meta = serde_json::Map::new();
    meta.insert("XAI_API_KEY".to_string(), serde_json::json!(12345_u64));

    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(meta))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("XAI_API_KEY"),
        "expected error mentioning XAI_API_KEY, got: {err}"
    );
}

#[tokio::test]
async fn set_session_mode_unknown_mode_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .set_session_mode(agent_client_protocol::SetSessionModeRequest::new(
            session_id,
            "nonexistent-mode",
        ))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("unknown mode"),
        "expected 'unknown mode' error, got: {err}"
    );
}

#[tokio::test]
async fn set_session_mode_default_succeeds() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_mode(agent_client_protocol::SetSessionModeRequest::new(
            session_id,
            "default",
        ))
        .await
        .expect("set_session_mode('default') should succeed");
}

#[tokio::test]
async fn set_session_mode_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let err = agent
        .set_session_mode(agent_client_protocol::SetSessionModeRequest::new(
            "no-such-session",
            "default",
        ))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not found"),
        "expected 'not found' for unknown session, got: {err}"
    );
}

#[tokio::test]
async fn set_session_config_option_unknown_id_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    let err = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            "no-such-session",
            "unknown_option",
            "value",
        ))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("not found"),
        "expected 'not found' for unknown session + unknown config_id, got: {err:?}"
    );
}

#[tokio::test]
async fn xai_prompt_timeout_secs_zero_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("XAI_BASE_URL");
        std::env::remove_var("XAI_MODELS");
        std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "0");
    }
    let agent =
        XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key");
    assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(300));
}

#[tokio::test]
async fn xai_models_all_malformed_falls_back_to_defaults() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_with_models("nocolon,alsomalformed").await;
    // When all entries are malformed the default models are used.
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap();
}

// ── system prompt / usage / tool calls ───────────────────────────────────────

/// SSE server that appends a usage chunk before `[DONE]`.
async fn fake_xai_sse_with_usage(text_chunks: &[&'static str]) -> String {
    let usage_chunk =
        "data: {\"type\":\"response.completed\",\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n";
    let body: String = text_chunks
        .iter()
        .map(|t| format!("data: {{\"id\":\"resp_test\",\"type\":\"message.delta\",\"delta\":{{\"type\":\"output_text\",\"text\":\"{t}\"}}}}\n\n"))
        .chain(std::iter::once(usage_chunk.to_string()))
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

/// SSE server that emits a Responses API `function_call` event (server-side tool
/// invocation) followed immediately by `[DONE]` with no text.
///
/// Server-side tools (web_search, x_search, code_interpreter) execute on the
/// xAI backend — the runner receives the function_call event for observability
/// and then the stream ends normally in one connection. No client round-trip.
async fn fake_xai_sse_with_tool_calls() -> String {
    let body = concat!(
        "data: {\"id\":\"resp_tool\",\"type\":\"function_call\",\"function_call\":{\"call_id\":\"call_test\",\"name\":\"web_search\",\"arguments\":\"{\\\"query\\\":\\\"rust\\\"}\"}}\n\n",
        "data: [DONE]\n\n",
    );

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

#[tokio::test]
async fn system_prompt_injected_as_first_message_in_request() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent_with_system_prompt(Some(&url), "You are a helpful assistant.").await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let input = captured[0]["input"].as_array().expect("input array");

    assert_eq!(input.len(), 2, "system + user");
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[0]["content"], "You are a helpful assistant.");
    assert_eq!(input[1]["role"], "user");
    assert_eq!(input[1]["content"], "hello");
}

#[tokio::test]
async fn system_prompt_absent_when_env_var_not_set() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let input = captured[0]["input"].as_array().expect("input array");

    assert_eq!(input.len(), 1, "only user message — no system message");
    assert_eq!(input[0]["role"], "user");
}

#[tokio::test]
async fn system_prompt_present_in_subsequent_turns() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["reply1"], vec!["reply2"]]).await;
    let agent = make_agent_with_system_prompt(Some(&url), "Be concise.").await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    agent
        .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new("turn 1"))]))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new("turn 2"))]))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    // Turn 1: system + user message (full history, no previous_response_id).
    let input1 = captured[0]["input"].as_array().expect("turn-1 input");
    assert_eq!(input1.len(), 2, "system + user");
    assert_eq!(input1[0]["role"], "system");
    assert_eq!(input1[0]["content"], "Be concise.");

    // Turn 2: stateful — only the new user message, plus previous_response_id.
    // The system prompt is already part of the xAI server's prior context.
    let input2 = captured[1]["input"].as_array().expect("turn-2 input");
    assert_eq!(input2.len(), 1, "only new user message on turn 2");
    assert_eq!(input2[0]["role"], "user");
    assert_eq!(input2[0]["content"], "turn 2");
    assert!(!captured[1]["previous_response_id"].is_null(), "turn 2 must have previous_response_id");
}

#[tokio::test]
async fn usage_chunk_is_handled_and_prompt_completes() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_with_usage(&["Hello"]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new("hi"))]))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content_str(), "Hello");
}

#[tokio::test]
async fn tool_call_response_ends_turn_without_updating_history() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_with_tool_calls().await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new("search"))]))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    // The user message is durably persisted before the xAI call. No text was
    // produced — only the user turn survives. The tool call is pending
    // client-side execution; the result arrives as the next user turn.
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "search");
}

// ── tool config options ───────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_exposes_all_tool_config_options_as_off() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let resp = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();

    let opts = resp.config_options.expect("config_options should be present");
    assert_eq!(opts.len(), 4, "expected 4 tool toggles: {opts:?}");
    let ids: Vec<String> = opts.iter().map(|o| o.id.to_string()).collect();
    assert!(ids.contains(&"web_search".to_string()));
    assert!(ids.contains(&"x_search".to_string()));
    assert!(ids.contains(&"code_interpreter".to_string()));
    assert!(ids.contains(&"file_search".to_string()));
    // All off by default.
    for opt in &opts {
        let current = match &opt.kind {
            agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
            _ => panic!("expected Select kind"),
        };
        assert_eq!(current, "off", "tool {} should be off by default", opt.id);
    }
}

#[tokio::test]
async fn load_session_exposes_tool_config_options() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent.load_session(LoadSessionRequest::new(sid.clone(), "/tmp")).await.unwrap();

    let opts = resp.config_options.expect("config_options should be present");
    assert_eq!(opts.len(), 4, "expected 4 tool toggle options");
}

#[tokio::test]
async fn enable_web_search_persists_and_is_reflected_in_response() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();

    assert_eq!(resp.config_options.len(), 4);
    let ws = resp.config_options.iter().find(|o| o.id.to_string() == "web_search")
        .expect("web_search option must be present");
    let current = match &ws.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on");
}

#[tokio::test]
async fn set_tool_unknown_value_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let err = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "turbo",
        ))
        .await
        .unwrap_err();

    assert!(err.message.contains("turbo"), "error should mention the bad value: {err:?}");
}

#[tokio::test]
async fn enabled_tools_are_sent_in_request_tools_array() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("search this"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let body = &captured[0];
    let tools = body["tools"].as_array()
        .expect("tools array must be present when web_search is enabled");
    assert!(
        tools.iter().any(|t| t["type"] == "web_search"),
        "web_search must be in tools array: {tools:?}"
    );
}

#[tokio::test]
async fn no_tools_omits_tools_field_from_request() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    // No tools enabled — default state.
    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("plain query"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let body = &captured[0];
    assert!(
        body["tools"].is_null(),
        "tools field should be absent when no tools are enabled: {body}"
    );
}

#[tokio::test]
async fn fork_session_inherits_enabled_tools() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let src_id = sess.session_id.to_string();

    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            src_id.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();

    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/tmp"))
        .await
        .unwrap();

    let opts = fork.config_options.expect("config_options should be present in fork");
    let ws = opts.iter().find(|o| o.id.to_string() == "web_search")
        .expect("web_search option must be present");
    let current = match &ws.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on", "forked session should inherit web_search=on");
}

// ── XAI_MAX_HISTORY_MESSAGES env var ─────────────────────────────────────────

async fn make_agent_with_max_history(base_url: Option<&str>, max: usize) -> XaiAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::set_var("XAI_MAX_HISTORY_MESSAGES", max.to_string());
        match base_url {
            Some(url) => std::env::set_var("XAI_BASE_URL", url),
            None => std::env::remove_var("XAI_BASE_URL"),
        }
    }
    XaiAgent::new_in_memory(fake_nats().await, AcpPrefix::new("test").unwrap(), "grok-3", "fake-key")
}

#[tokio::test]
async fn xai_max_history_messages_default_is_20() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    assert_eq!(agent.test_max_history_messages(), 20);
}

#[tokio::test]
async fn xai_max_history_messages_zero_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_with_max_history(None, 0).await;
    assert_eq!(agent.test_max_history_messages(), 20);
}

#[tokio::test]
async fn xai_max_history_messages_custom_value_accepted() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_with_max_history(None, 10).await;
    assert_eq!(agent.test_max_history_messages(), 10);
}

#[tokio::test]
async fn history_is_truncated_after_exceeding_max() {
    let _guard = env_lock().lock().unwrap();
    // limit = 4 messages (2 exchanges). After 3 exchanges the oldest pair is dropped.
    let url = fake_xai_sse_multi(3, vec!["reply"]).await;
    let agent = make_agent_with_max_history(Some(&url), 4).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    for i in 1..=3u32 {
        agent
            .prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Text(TextContent::new(format!("turn {i}")))],
            ))
            .await
            .unwrap();
    }

    // After 3 turns (6 messages), with max=4, only the last 4 should remain.
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 4, "expected 4 messages, got {}: {history:?}", history.len());
    assert_eq!(history[0].content_str(), "turn 2", "oldest pair should have been dropped");
    assert_eq!(history[2].content_str(), "turn 3");
}

#[tokio::test]
async fn history_truncation_drops_oldest_pair_first() {
    let _guard = env_lock().lock().unwrap();
    // limit = 2 messages (1 exchange). Each new turn replaces the previous.
    let url = fake_xai_sse_multi(3, vec!["reply"]).await;
    let agent = make_agent_with_max_history(Some(&url), 2).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    for i in 1..=3u32 {
        agent
            .prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Text(TextContent::new(format!("turn {i}")))],
            ))
            .await
            .unwrap();
    }

    // Only the last exchange should remain.
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content_str(), "turn 3");
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

// ── coverage gaps ─────────────────────────────────────────────────────────────

// 1. x_search tool enabled sends it in the tools array.
#[tokio::test]
async fn x_search_tool_enabled_is_included_in_tools_array() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "x_search",
            "on",
        ))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("search X"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let tools = captured[0]["tools"].as_array()
        .expect("tools array must be present when x_search is enabled");
    assert!(
        tools.iter().any(|t| t["type"] == "x_search"),
        "x_search must be in tools array: {tools:?}"
    );
}

// 2. set_session_config_option with an unknown session returns an error.
#[tokio::test]
async fn set_session_config_option_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let err = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            "no-such-session",
            "web_search",
            "on",
        ))
        .await
        .unwrap_err();

    assert!(
        err.message.contains("not found"),
        "error should mention 'not found': {err:?}"
    );
}

// 3. History is NOT truncated when len == max (boundary: exactly at the limit).
#[tokio::test]
async fn history_at_exactly_the_limit_is_not_truncated() {
    let _guard = env_lock().lock().unwrap();
    // max = 4 messages. After exactly 2 turns (4 messages) nothing should be dropped.
    let url = fake_xai_sse_multi(2, vec!["reply"]).await;
    let agent = make_agent_with_max_history(Some(&url), 4).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    for i in 1..=2u32 {
        agent
            .prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Text(TextContent::new(format!("turn {i}")))],
            ))
            .await
            .unwrap();
    }

    let history = agent.test_session_history(&sid).await;
    assert_eq!(
        history.len(),
        4,
        "history should be exactly 4 (no truncation at limit): {history:?}"
    );
    assert_eq!(history[0].content_str(), "turn 1");
    assert_eq!(history[2].content_str(), "turn 2");
}

// 4. Odd max_history_messages: trim removes complete pairs, so with max=3 and
//    4 messages accumulated, one full pair is dropped and 2 messages remain.
#[tokio::test]
async fn history_truncation_rounds_up_to_even_for_odd_max() {
    let _guard = env_lock().lock().unwrap();
    // max=3 (odd). After 2 turns we have 4 messages. trim_history removes the
    // oldest complete user/assistant pair, leaving 2 messages.
    let url = fake_xai_sse_multi(2, vec!["reply"]).await;
    let agent = make_agent_with_max_history(Some(&url), 3).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    for i in 1..=2u32 {
        agent
            .prompt(PromptRequest::new(
                sid.clone(),
                vec![ContentBlock::Text(TextContent::new(format!("turn {i}")))],
            ))
            .await
            .unwrap();
    }

    let history = agent.test_session_history(&sid).await;
    assert_eq!(
        history.len(),
        2,
        "odd max=3: oldest pair dropped, 2 messages remain: {history:?}"
    );
    // Only the last pair (turn 2 user + assistant reply) should survive.
    assert_eq!(history[0].content_str(), "turn 2");
}

// 5. Multiple function_call events in one turn: agent handles them without
//    panicking, ends the turn, and does not update history (no text produced).
#[tokio::test]
async fn multiple_tool_calls_in_one_turn_do_not_panic() {
    let _guard = env_lock().lock().unwrap();

    // Responses API: two function_call events, then [DONE] — no text.
    // Server-side tools are handled on the xAI backend; single connection.
    let body = concat!(
        "data: {\"id\":\"resp_tools\",\"type\":\"function_call\",\"function_call\":{\"call_id\":\"call_a\",\"name\":\"web_search\",\"arguments\":\"{\\\"q\\\":\\\"rust\\\"}\"}}\n\n",
        "data: {\"id\":\"resp_tools\",\"type\":\"function_call\",\"function_call\":{\"call_id\":\"call_b\",\"name\":\"code_interpreter\",\"arguments\":\"{\\\"code\\\":\\\"2+2\\\"}\"}}\n\n",
        "data: [DONE]\n\n",
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");

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

    let agent = make_agent(Some(&url)).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("use two tools"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    // No text was produced — only the user turn survives in history.
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "use two tools");
}

// set_session_config_option with unknown config_id returns current state of
// all known options (per ACP spec), not an empty list.
#[tokio::test]
async fn set_session_config_option_unknown_id_returns_current_state() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    // Enable web_search so we can verify the current state is returned.
    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();

    // Now call with an unknown config_id.
    let resp = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "unknown_option",
            "value",
        ))
        .await
        .unwrap();

    // Should return all known tool options.
    assert_eq!(resp.config_options.len(), 4, "should return current state of all known tool options");
    let ws = resp.config_options.iter().find(|o| o.id.to_string() == "web_search")
        .expect("web_search option must be present");
    let current = match &ws.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on", "web_search should still be 'on'");
}

// ── additional coverage gaps ──────────────────────────────────────────────────

// load_session reflects the current_value after a tool is enabled.
#[tokio::test]
async fn load_session_reflects_updated_tool_state() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "code_interpreter",
            "on",
        ))
        .await
        .unwrap();

    let loaded = agent.load_session(LoadSessionRequest::new(sid.clone(), "/tmp")).await.unwrap();
    let opts = loaded.config_options.expect("config_options should be present");
    assert_eq!(opts.len(), 4);
    let ci = opts.iter().find(|o| o.id.to_string() == "code_interpreter")
        .expect("code_interpreter option must be present");
    let current = match &ci.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on", "load_session should reflect the persisted tool state");
}

// Enabling then disabling a tool removes it from the tools array in subsequent requests.
#[tokio::test]
async fn disabling_tool_removes_it_from_request() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    // Enable web_search, then disable it.
    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "on"))
        .await
        .unwrap();
    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "off"))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("query"))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    assert!(
        captured[0]["tools"].is_null(),
        "tools must be absent after disabling all tools: {}",
        captured[0]
    );
}

// close_session cancels an in-flight prompt and the session is deleted.
#[tokio::test]
async fn close_session_cancels_in_flight_prompt() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_slow("partial").await;
    let agent = std::sync::Arc::new(make_agent(Some(&url)).await);

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let agent2 = std::sync::Arc::clone(&agent);
    let sid = session_id.clone();

    let (prompt_result, _) = tokio::join!(
        agent.prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("test"))],
        )),
        async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            agent2.close_session(CloseSessionRequest::new(sid)).await.unwrap();
        }
    );

    // Prompt must have been cancelled (not a timeout or error).
    assert_eq!(prompt_result.unwrap().stop_reason, StopReason::Cancelled);

    // Session must be gone.
    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
    assert!(list.sessions.is_empty(), "session must be deleted by close_session");
}

// close_session with an unknown session ID is a no-op — returns Ok.
#[tokio::test]
async fn close_session_unknown_session_is_noop() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(None).await;
    agent
        .close_session(CloseSessionRequest::new("no-such-session"))
        .await
        .expect("close_session on unknown id should succeed silently");
}

// Prompt timeout with NO text received before deadline — history must stay empty.
// Distinct from prompt_times_out_when_server_stops_responding, which sends a
// text chunk before holding (so that test verifies history IS updated on timeout).
#[tokio::test]
async fn prompt_timeout_with_no_text_does_not_update_history() {
    let _guard = env_lock().lock().unwrap();

    // Server sends headers only — no text chunks, connection held open.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            writer
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await
                .ok();
            writer.flush().await.ok();
            // Hold open without sending any SSE data.
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    let agent = make_agent_with_timeout(Some(&url), 1).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::Cancelled);

    // The user message is durably persisted before the xAI call. No text was
    // produced before the timeout — only the user turn survives in history.
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "hello");
}

// Fork inherits the parent's system_prompt: subsequent prompts on the forked
// session must include the system message in the HTTP request body.
#[tokio::test]
async fn fork_session_inherits_system_prompt() {
    let _guard = env_lock().lock().unwrap();
    // Two connections: one for the parent prompt, one for the fork prompt.
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["parent reply"], vec!["fork reply"]]).await;
    let agent = make_agent_with_system_prompt(Some(&url), "Be concise.").await;

    // Create parent and prompt once to build history.
    let src = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let src_id = src.session_id.to_string();
    agent
        .prompt(PromptRequest::new(
            src_id.clone(),
            vec![ContentBlock::Text(TextContent::new("parent question"))],
        ))
        .await
        .unwrap();

    // Fork — must inherit system_prompt from parent session data.
    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    agent
        .prompt(PromptRequest::new(
            fork_id.clone(),
            vec![ContentBlock::Text(TextContent::new("fork question"))],
        ))
        .await
        .unwrap();

    // Fork clears last_response_id (fork starts fresh), so the fork's first prompt
    // sends full history. Inspect the fork's HTTP request: input[0] must be the
    // system prompt, followed by the inherited history + the new user message.
    let captured = bodies.lock().unwrap();
    let fork_input = captured[1]["input"].as_array().expect("fork input array");
    assert_eq!(
        fork_input[0]["role"], "system",
        "first input item of fork prompt must be system: {fork_input:?}"
    );
    assert_eq!(fork_input[0]["content"], "Be concise.");
}

// ── ContentBlock::ResourceLink and ContentBlock::Resource ─────────────────────

// ResourceLink: the URI and name must be forwarded to xAI as text context.
#[tokio::test]
async fn resource_link_is_included_as_text_in_request() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::ResourceLink(
                ResourceLink::new("README", "file:///project/README.md"),
            )],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let input = captured[0]["input"].as_array().expect("input array");
    let content = input.last().unwrap()["content"].as_str().unwrap();
    assert!(
        content.contains("README") && content.contains("file:///project/README.md"),
        "ResourceLink must be forwarded as text: {content:?}"
    );
}

// EmbeddedResource with text content: the text must be passed through directly.
#[tokio::test]
async fn embedded_text_resource_is_included_as_text_in_request() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new("fn main() {}", "file:///src/main.rs"),
                ),
            ))],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let input = captured[0]["input"].as_array().expect("input array");
    let content = input.last().unwrap()["content"].as_str().unwrap();
    assert_eq!(content, "fn main() {}");
}

// Mixed prompt: Text + ResourceLink blocks are joined with "\n".
#[tokio::test]
async fn mixed_text_and_resource_link_blocks_are_joined() {
    let _guard = env_lock().lock().unwrap();
    let (url, bodies) = fake_xai_sse_recording(vec![vec!["ok"]]).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![
                ContentBlock::Text(TextContent::new("Please review:")),
                ContentBlock::ResourceLink(ResourceLink::new("main.rs", "file:///src/main.rs")),
            ],
        ))
        .await
        .unwrap();

    let captured = bodies.lock().unwrap();
    let input = captured[0]["input"].as_array().expect("input array");
    let content = input.last().unwrap()["content"].as_str().unwrap();
    assert!(content.starts_with("Please review:"), "text block must come first: {content:?}");
    assert!(content.contains("main.rs"), "resource link must be included: {content:?}");
}

// ── response.error mid-stream event ──────────────────────────────────────────

/// SSE server: emits a `response.error` event (mid-stream error).
async fn fake_xai_sse_response_error(code: &'static str, message: &'static str) -> String {
    let body = format!(
        "data: {{\"type\":\"response.error\",\"error\":{{\"code\":\"{code}\",\"message\":\"{message}\"}}}}\n\ndata: [DONE]\n\n"
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(), body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

/// SSE server: emits `response.completed` with `status: "cancelled"`.
async fn fake_xai_sse_cancelled() -> String {
    let body = concat!(
        "data: {\"id\":\"resp_cancelled\",\"type\":\"response.completed\",\"response\":{\"status\":\"cancelled\"}}\n\n",
        "data: [DONE]\n\n",
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(), body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

#[tokio::test]
async fn response_error_event_surfaces_as_acp_error() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_response_error("content_policy", "Request blocked by content policy").await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("test"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError.into());
    // Orphaned user message must be compensated.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "history must be empty after response.error: {history:?}");
}

#[tokio::test]
async fn xai_server_cancelled_surfaces_as_acp_error() {
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_sse_cancelled().await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("test"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError.into());
    // Orphaned user message must be compensated — server cancelled the request.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "history must be empty after server-side cancel: {history:?}");
}

#[tokio::test]
async fn api_401_error_surfaces_as_acp_error_without_retry() {
    // A 401 error must surface as ACP error. The 4xx-skip guard in the stale-ID
    // retry path ensures only one API call is made (not two). We verify the
    // error contract here; the retry-count invariant is validated by the single
    // fake_xai_error server accepting only one connection.
    let _guard = env_lock().lock().unwrap();
    let url = fake_xai_error(401).await;
    let agent = make_agent(Some(&url)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("ping"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError.into());
    // History compensated away — turn failed entirely.
    let history = agent.test_session_history(&session_id).await;
    assert!(history.is_empty(), "history must be empty after 401 error: {history:?}");
}

// ── Incomplete continuation ───────────────────────────────────────────────────

#[tokio::test]
async fn incomplete_max_output_tokens_continues_and_assembles_text() {
    // Exercises the outer agentic loop continuation path:
    //   1st request → incomplete (max_output_tokens) with partial text
    //   2nd request → continuation with empty input + previous_response_id
    // Verifies: exactly 2 HTTP requests, second carries previous_response_id and
    // empty input array, and the assembled assistant text is saved in history.
    let _guard = env_lock().lock().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");
    let captured: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);

    tokio::spawn(async move {
        // First connection: partial text, then incomplete.
        let first = concat!(
            "data: {\"id\":\"resp_part\",\"type\":\"message.delta\",\"delta\":{\"type\":\"output_text\",\"text\":\"Hello \"}}\n\n",
            "data: {\"id\":\"resp_part\",\"type\":\"response.completed\",\"response\":{\"id\":\"resp_part\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            "data: [DONE]\n\n",
        );
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut content_length: usize = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_err() { break; }
                let t = line.trim_end_matches('\n').trim_end_matches('\r');
                if t.is_empty() { break; }
                if let Some(r) = t.to_lowercase().strip_prefix("content-length:") {
                    content_length = r.trim().parse().unwrap_or(0);
                }
            }
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).await.ok();
                let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                captured_clone.lock().unwrap().push(json);
            }
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                first.len(), first,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }

        // Second connection: continuation text, then complete.
        let second = concat!(
            "data: {\"id\":\"resp_cont\",\"type\":\"message.delta\",\"delta\":{\"type\":\"output_text\",\"text\":\"world\"}}\n\n",
            "data: {\"id\":\"resp_cont\",\"type\":\"response.completed\",\"response\":{\"id\":\"resp_cont\",\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n",
        );
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut content_length: usize = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_err() { break; }
                let t = line.trim_end_matches('\n').trim_end_matches('\r');
                if t.is_empty() { break; }
                if let Some(r) = t.to_lowercase().strip_prefix("content-length:") {
                    content_length = r.trim().parse().unwrap_or(0);
                }
            }
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).await.ok();
                let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                captured_clone.lock().unwrap().push(json);
            }
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                second.len(), second,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });

    let agent = make_agent(Some(&url)).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("say hello world"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let reqs = captured.lock().unwrap();
    assert_eq!(reqs.len(), 2, "expected exactly 2 HTTP requests");

    // First request has a user message and no previous_response_id.
    let first_input = reqs[0]["input"].as_array().expect("input must be array");
    assert!(
        first_input.iter().any(|item| item["role"].as_str() == Some("user")),
        "first request must carry the user message"
    );
    assert!(
        reqs[0].get("previous_response_id").and_then(|v| v.as_str()).is_none(),
        "first request must not have previous_response_id"
    );

    // Continuation: empty input + previous_response_id from the incomplete response.
    let second_input = reqs[1]["input"].as_array().expect("second input must be array");
    assert!(
        second_input.is_empty(),
        "max_output_tokens continuation must send empty input, got {second_input:?}"
    );
    assert_eq!(
        reqs[1]["previous_response_id"].as_str(),
        Some("resp_part"),
        "continuation must reference the incomplete response's ID"
    );

    // History: user turn + assembled assistant text from both streams.
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content_str(), "Hello world", "assembled text must concatenate both chunks");
}

// ── tool-call → Failed on stream error ───────────────────────────────────────

#[tokio::test]
async fn tool_call_then_response_failed_compensates_history() {
    // Verifies 121ce6c: when response.failed arrives after a function_call event,
    // the agent must return an error and compensate the orphaned user message.
    // Tool call notifications sent to NATS are fire-and-forget and cannot be
    // observed here (fake_nats drops PUBs), but the history/error contract can.
    let _guard = env_lock().lock().unwrap();

    let body = concat!(
        "data: {\"id\":\"resp_toolerr\",\"type\":\"function_call\",\"function_call\":{\"call_id\":\"call_1\",\"name\":\"web_search\",\"arguments\":\"{}\"}}\n\n",
        "data: {\"id\":\"resp_toolerr\",\"type\":\"response.failed\"}\n\n",
        "data: [DONE]\n\n",
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/v1");

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            drain_request(&mut reader).await;
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(), body,
            );
            writer.write_all(http.as_bytes()).await.ok();
        }
    });

    let agent = make_agent(Some(&url)).await;
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("search for something"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError.into());
    // Orphaned user message must be compensated — turn failed after tool call.
    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "history must be empty after response.failed during tool call: {history:?}"
    );
}

