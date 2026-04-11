//! Integration tests for `XaiAgent`.
//!
//! All tests use an in-process `MockXaiHttpClient` — no TCP servers are needed
//! for the xAI HTTP layer. A minimal fake NATS TCP stub is still used to obtain
//! a real `async_nats::Client` (fire-and-forget PUBs are silently dropped).
//!
//! Tests that need to inspect HTTP request parameters access `mock.calls` after
//! each prompt. Tests that need request body control pre-load responses with
//! `mock.push_response(events)` or `mock.push_slow_response(first_event)`.
//!
//! Run with: `cargo test -p trogon-xai-runner --features test-helpers`

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_client_protocol::{
    Agent, AuthMethod, AuthenticateRequest, CancelNotification, CloseSessionRequest, ContentBlock,
    EmbeddedResource, EmbeddedResourceResource, ForkSessionRequest, InitializeRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, ProtocolVersion,
    ResourceLink, ResumeSessionRequest, SetSessionConfigOptionRequest, SetSessionModelRequest,
    StopReason, TextContent, TextResourceContents,
};
use trogon_xai_runner::{FinishReason, MockSessionNotifier, MockXaiHttpClient, XaiAgent, XaiEvent};

// ── env-var lock ──────────────────────────────────────────────────────────────

/// All tests must hold this lock for the duration of env-var writes + agent
/// construction. `make_agent` resets env vars at entry.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ── test agent type alias ─────────────────────────────────────────────────────

type TestAgent = XaiAgent<Arc<MockXaiHttpClient>, MockSessionNotifier>;

// ── agent factory ─────────────────────────────────────────────────────────────

/// Build a `TestAgent` with the given mock HTTP client. Resets all env vars to
/// clean defaults. Caller must hold `env_lock()`.
async fn make_agent(mock: Arc<MockXaiHttpClient>) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "fake-key", mock)
}

/// Like `make_agent` but passes an empty api_key (no server-wide key).
async fn make_agent_no_key(mock: Arc<MockXaiHttpClient>) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "", mock)
}

/// Like `make_agent` but also sets `XAI_MODELS`. Caller must hold `env_lock()`.
async fn make_agent_with_models(mock: Arc<MockXaiHttpClient>, models_env: &str) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
        std::env::set_var("XAI_MODELS", models_env);
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "fake-key", mock)
}

/// Like `make_agent` but sets `XAI_PROMPT_TIMEOUT_SECS`. Caller must hold `env_lock()`.
async fn make_agent_with_timeout(mock: Arc<MockXaiHttpClient>, timeout_secs: u64) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
        std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", timeout_secs.to_string());
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "fake-key", mock)
}

/// Like `make_agent` but also sets `XAI_SYSTEM_PROMPT`. Caller must hold `env_lock()`.
async fn make_agent_with_system_prompt(
    mock: Arc<MockXaiHttpClient>,
    system_prompt: &str,
) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
        std::env::set_var("XAI_SYSTEM_PROMPT", system_prompt);
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "fake-key", mock)
}

/// Like `make_agent` but sets `XAI_MAX_HISTORY_MESSAGES`. Caller must hold `env_lock()`.
async fn make_agent_with_max_history(mock: Arc<MockXaiHttpClient>, max: usize) -> TestAgent {
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_TURNS");
        std::env::remove_var("XAI_BASE_URL");
        std::env::set_var("XAI_MAX_HISTORY_MESSAGES", max.to_string());
    }
    XaiAgent::new_in_memory(MockSessionNotifier::new(), "grok-3", "fake-key", mock)
}

// ── mock event helpers ────────────────────────────────────────────────────────

/// A successful response with text chunks and a response ID.
/// The ID is used as `last_response_id` for the next turn (stateful multi-turn).
fn text_response_with_id(text: &str, id: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::ResponseId { id: id.to_string() },
        XaiEvent::TextDelta {
            text: text.to_string(),
        },
        XaiEvent::Done,
    ]
}

/// A successful response with multiple text chunks but no stored response ID.
fn text_response(chunks: &[&str]) -> Vec<XaiEvent> {
    let mut v: Vec<XaiEvent> = chunks
        .iter()
        .map(|t| XaiEvent::TextDelta {
            text: t.to_string(),
        })
        .collect();
    v.push(XaiEvent::Done);
    v
}

/// A response that yields only `[DONE]` with no text (empty assistant reply).
fn done_only() -> Vec<XaiEvent> {
    vec![XaiEvent::Done]
}

/// An error response (surfaces as InternalError to the ACP client).
/// Message must NOT contain "xAI API error 4" — otherwise the stale-ID retry
/// guard skips the retry. Use `client_error_response` for 4xx errors.
fn error_response() -> Vec<XaiEvent> {
    vec![XaiEvent::Error {
        message: "xAI API error 500: server error".to_string(),
    }]
}

/// A 4xx error response — no stale-ID retry is attempted.
fn client_error_response() -> Vec<XaiEvent> {
    vec![XaiEvent::Error {
        message: "xAI API error 401: Unauthorized".to_string(),
    }]
}

/// A response with token usage and text.
fn text_with_usage(text: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::TextDelta {
            text: text.to_string(),
        },
        XaiEvent::Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
        },
        XaiEvent::Done,
    ]
}

/// A function-call-only response (no text) — server-side tool execution.
fn function_call_response(call_id: &str, name: &str, args: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::ResponseId {
            id: "resp_tool".to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        },
        XaiEvent::Done,
    ]
}

/// A response where the xAI server cancelled the request (content policy etc.).
fn server_cancelled_response() -> Vec<XaiEvent> {
    vec![
        XaiEvent::ResponseId {
            id: "resp_cancelled".to_string(),
        },
        XaiEvent::Finished {
            reason: FinishReason::Cancelled,
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]
}

/// A response where the model failed (response.failed or response.error).
fn server_failed_response(msg: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::Error {
            message: msg.to_string(),
        },
        XaiEvent::Done,
    ]
}

/// Incomplete response due to max_output_tokens with a text chunk.
fn incomplete_max_output_tokens(text: &str, id: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::TextDelta {
            text: text.to_string(),
        },
        XaiEvent::ResponseId { id: id.to_string() },
        XaiEvent::Finished {
            reason: FinishReason::Incomplete,
            incomplete_reason: Some("max_output_tokens".to_string()),
        },
        XaiEvent::Done,
    ]
}

/// Incomplete response due to max_turns (no text).
fn incomplete_max_turns(id: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::ResponseId { id: id.to_string() },
        XaiEvent::Finished {
            reason: FinishReason::Incomplete,
            incomplete_reason: Some("max_turns".to_string()),
        },
        XaiEvent::Done,
    ]
}

/// Continuation response (after incomplete): text + completion.
fn continuation_response(text: &str, id: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::TextDelta {
            text: text.to_string(),
        },
        XaiEvent::ResponseId { id: id.to_string() },
        XaiEvent::Finished {
            reason: FinishReason::Completed,
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]
}

/// Build a meta map with a single XAI_API_KEY entry.
fn api_key_meta(key: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("XAI_API_KEY".to_string(), serde_json::json!(key));
    m
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_creates_session_with_correct_model_state() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let resp = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();

    assert!(!resp.session_id.to_string().is_empty());
    let models = resp.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3");

    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert_eq!(list.sessions.len(), 1);
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_is_empty_before_any_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert!(list.sessions.is_empty());
}

#[tokio::test]
async fn list_sessions_sorted_by_session_id() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    agent
        .new_session(NewSessionRequest::new("/a"))
        .await
        .unwrap();
    agent
        .new_session(NewSessionRequest::new("/b"))
        .await
        .unwrap();
    agent
        .new_session(NewSessionRequest::new("/c"))
        .await
        .unwrap();

    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert_eq!(list.sessions.len(), 3);

    let ids: Vec<String> = list
        .sessions
        .iter()
        .map(|s| s.session_id.to_string())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "sessions must be sorted by id");
}

#[tokio::test]
async fn list_sessions_returns_correct_cwd_for_each_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

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

    assert_eq!(cwds, vec!["/project/alpha", "/project/beta", "/tmp"]);
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_returns_error_for_unknown_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let err = agent
        .load_session(LoadSessionRequest::new("no-such-id", "/tmp"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"), "error: {}", err.message);
}

#[tokio::test]
async fn load_session_returns_correct_state_for_existing_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let new = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let new = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = new.session_id.to_string();

    agent
        .close_session(CloseSessionRequest::new(session_id.clone()))
        .await
        .unwrap();

    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert!(list.sessions.is_empty());
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test]
async fn resume_session_returns_error_for_unknown_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let err = agent
        .resume_session(ResumeSessionRequest::new("no-such-id", "/tmp"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn resume_session_succeeds_for_existing_session() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let new = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let new = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = new.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(
            session_id.clone(),
            "grok-3-mini",
        ))
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let new = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let src = agent
        .new_session(NewSessionRequest::new("/src"))
        .await
        .unwrap();
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

    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert_eq!(list.sessions.len(), 2);
}

#[tokio::test]
async fn fork_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let err = agent
        .fork_session(ForkSessionRequest::new("no-such-id", "/fork"))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn fork_session_clones_conversation_history() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["Hi"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let src = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let src_id = src.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("hello"))];
    agent
        .prompt(PromptRequest::new(src_id.clone(), content))
        .await
        .unwrap();

    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    assert_ne!(fork_id, src_id);
    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert_eq!(list.sessions.len(), 2);

    let src_history = agent.test_session_history(&src_id).await;
    let fork_history = agent.test_session_history(&fork_id).await;
    assert_eq!(src_history.len(), 2, "source: user + assistant");
    assert_eq!(fork_history.len(), 2, "fork should inherit history");
    assert_eq!(fork_history[0].content_str(), src_history[0].content_str());
}

// ── prompt ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_unknown_session_id_returns_not_found() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    // None of these IDs have an associated session, so the store lookup fails.
    for bad_id in &[
        "foo.bar",
        "foo*bar",
        "foo>bar",
        "foo bar",
        "séssion",
        "",
        "no-such-id",
    ] {
        let err = agent
            .prompt(PromptRequest::new(
                *bad_id,
                vec![ContentBlock::Text(TextContent::new("hi"))],
            ))
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            agent_client_protocol::ErrorCode::ResourceNotFound.into(),
            "expected ResourceNotFound for session_id {:?}, got: {:?}",
            bad_id,
            err,
        );
    }
}

#[tokio::test]
async fn prompt_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
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
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["Hello", " world"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["First reply"]));
    let agent = make_agent(Arc::clone(&mock)).await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let c1 = vec![ContentBlock::Text(TextContent::new("first turn"))];
    agent
        .prompt(PromptRequest::new(session_id.clone(), c1))
        .await
        .unwrap();

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content_str(), "first turn");
    assert_eq!(history[1].content_str(), "First reply");
}

#[tokio::test]
async fn prompt_with_api_error_returns_acp_error() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(client_error_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("ping"))];
    let err = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::InternalError.into()
    );

    // User message is compensated away on error.
    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "history should be empty after xAI error: {history:?}"
    );
}

#[tokio::test]
async fn prompt_with_done_only_response_does_not_update_history() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(done_only());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("ping"))];
    agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "ping");
}

#[tokio::test]
async fn prompt_retry_after_incomplete_turn_does_not_duplicate_user_message() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["the reply"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    // Simulate a crash: manually plant the user message in history without a
    // corresponding assistant reply.
    {
        let mut data = agent.test_session_history(&session_id).await;
        data.push(trogon_xai_runner::Message::user("hello"));
        agent.test_set_session_history(&session_id, data).await;
    }

    // Retry with the same message — must be treated as a resume, not a fresh turn.
    let resp = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(
        history.len(),
        2,
        "expected user + assistant, got: {history:?}"
    );
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "hello");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content_str(), "the reply");

    // The mock must have been called with exactly one user message (no duplicate).
    let calls = mock.calls.lock().unwrap();
    let user_msgs: Vec<_> = calls[0].input.iter().filter(|i| i.role == "user").collect();
    assert_eq!(
        user_msgs.len(),
        1,
        "xAI request must have exactly one user message"
    );
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_unknown_session_is_a_noop() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    agent
        .cancel(CancelNotification::new("no-such-id"))
        .await
        .unwrap();
}

#[tokio::test]
async fn cancel_interrupts_in_flight_prompt() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_slow_response(XaiEvent::TextDelta {
        text: "partial".to_string(),
    });
    let agent = std::sync::Arc::new(make_agent(Arc::clone(&mock)).await);

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let agent_cancel = std::sync::Arc::clone(&agent);
    let cancel_session_id = session_id.clone();

    let content = vec![ContentBlock::Text(TextContent::new("test"))];
    let (prompt_result, _) = tokio::join!(
        agent.prompt(PromptRequest::new(session_id.clone(), content)),
        async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            agent_cancel
                .cancel(CancelNotification::new(cancel_session_id))
                .await
                .unwrap();
        }
    );

    let resp = prompt_result.unwrap();
    assert_eq!(resp.stop_reason, StopReason::Cancelled);

    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "canceled prompt must not update history"
    );
}

#[tokio::test]
async fn cancel_channels_entry_is_cleaned_up_after_prompt() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["hello"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    assert_eq!(agent.test_cancel_channels_len().await, 0);

    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hi"))],
        ))
        .await
        .unwrap();

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
    let agent = make_agent_with_models(
        Arc::new(MockXaiHttpClient::new()),
        "model-a:Model A,model-b:Model B",
    )
    .await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

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
        .set_session_model(SetSessionModelRequest::new(
            session_id.clone(),
            "grok-3-mini",
        ))
        .await
        .unwrap_err();
    assert!(err.message.contains("unknown model"));
}

#[tokio::test]
async fn xai_models_env_var_malformed_entries_are_skipped() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_with_models(
        Arc::new(MockXaiHttpClient::new()),
        "valid:Valid,malformed,another:Another",
    )
    .await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "valid"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "another"))
        .await
        .unwrap();
    let err = agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "malformed"))
        .await
        .unwrap_err();
    assert!(err.message.contains("unknown model"));
}

#[tokio::test]
async fn xai_models_default_when_env_var_not_set() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(
            session_id.clone(),
            "grok-3-mini",
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn xai_models_default_model_auto_added_when_absent_from_list() {
    let _guard = env_lock().lock().unwrap();
    let agent =
        make_agent_with_models(Arc::new(MockXaiHttpClient::new()), "alpha:Alpha,beta:Beta").await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let models = sess.models.expect("models should be present");
    assert_eq!(models.current_model_id.to_string(), "grok-3");

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .expect("auto-added default model must be selectable");
}

// ── partial SSE across chunks (SSE parsing tested via agent round-trip) ────────

#[tokio::test]
async fn text_is_assembled_from_multiple_deltas() {
    // The SSE chunked-parse test is now covered by unit tests in client.rs.
    // This variant verifies the agent correctly concatenates multiple TextDelta
    // events from the mock into a single history entry.
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["Hel", "lo"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("hi"))];
    let resp = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content_str(), "Hello");
}

// ── prompt timeout ────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_times_out_when_server_stops_responding() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // Yield "partial" then block forever.
    mock.push_slow_response(XaiEvent::TextDelta {
        text: "partial".to_string(),
    });
    // 1-second timeout — the mock holds the stream indefinitely after the first event.
    let agent = make_agent_with_timeout(Arc::clone(&mock), 1).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let content = vec![ContentBlock::Text(TextContent::new("hello"))];
    let start = std::time::Instant::now();
    let resp = agent
        .prompt(PromptRequest::new(session_id.clone(), content))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.stop_reason, StopReason::Cancelled);
    assert!(
        elapsed < Duration::from_secs(5),
        "timed out too late: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "timed out too early: {elapsed:?}"
    );

    // Partial text received before timeout is preserved.
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content_str(), "partial");
}

// ── history as context ────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_includes_history_as_context_on_subsequent_turns() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response_with_id("First reply", "resp_0"));
    mock.push_response(text_response(&["Second reply"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let c1 = vec![ContentBlock::Text(TextContent::new("first question"))];
    agent
        .prompt(PromptRequest::new(session_id.clone(), c1))
        .await
        .unwrap();

    let c2 = vec![ContentBlock::Text(TextContent::new("second question"))];
    agent
        .prompt(PromptRequest::new(session_id.clone(), c2))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected two recorded requests");

    // Turn 1: full history (just the user message — no prior context).
    assert_eq!(
        calls[0].input.len(),
        1,
        "turn 1 should have exactly one input item"
    );
    assert_eq!(calls[0].input[0].role, "user");
    assert_eq!(calls[0].input[0].content, "first question");
    assert!(
        calls[0].previous_response_id.is_none(),
        "turn 1 must not send previous_response_id"
    );

    // Turn 2: stateful via previous_response_id — only the new user message.
    assert_eq!(
        calls[1].input.len(),
        1,
        "turn 2 should send only the new user message"
    );
    assert_eq!(calls[1].input[0].role, "user");
    assert_eq!(calls[1].input[0].content, "second question");
    assert_eq!(
        calls[1].previous_response_id.as_deref(),
        Some("resp_0"),
        "turn 2 must reference turn 1's response id"
    );
}

// ── concurrent session isolation ─────────────────────────────────────────────

#[tokio::test]
async fn concurrent_sessions_do_not_cross_contaminate() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["session-reply"]));
    mock.push_response(text_response(&["session-reply"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess_a = agent
        .new_session(NewSessionRequest::new("/tmp/a"))
        .await
        .unwrap();
    let sess_b = agent
        .new_session(NewSessionRequest::new("/tmp/b"))
        .await
        .unwrap();
    let id_a = sess_a.session_id.to_string();
    let id_b = sess_b.session_id.to_string();

    let c_a = vec![ContentBlock::Text(TextContent::new("question-a"))];
    let c_b = vec![ContentBlock::Text(TextContent::new("question-b"))];

    let (r_a, r_b) = tokio::join!(
        agent.prompt(PromptRequest::new(id_a.clone(), c_a)),
        agent.prompt(PromptRequest::new(id_b.clone(), c_b)),
    );
    assert_eq!(r_a.unwrap().stop_reason, StopReason::EndTurn);
    assert_eq!(r_b.unwrap().stop_reason, StopReason::EndTurn);

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

#[tokio::test]
async fn initialize_declares_embedded_context_prompt_capability() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let resp = agent
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();
    assert!(
        resp.agent_capabilities.prompt_capabilities.embedded_context,
        "agent must declare embedded_context=true"
    );
}

#[tokio::test]
async fn initialize_always_advertises_xai_api_key_method() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let resp = agent
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();
    assert!(
        resp.auth_methods
            .iter()
            .any(|m| m.id().to_string() == "xai-api-key"),
        "expected 'xai-api-key' in auth_methods",
    );
}

#[tokio::test]
async fn initialize_advertises_agent_method_only_when_server_key_configured() {
    let _guard = env_lock().lock().unwrap();

    let with_key = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let resp = with_key
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();
    assert!(
        resp.auth_methods
            .iter()
            .any(|m| m.id().to_string() == "agent")
    );

    let no_key = make_agent_no_key(Arc::new(MockXaiHttpClient::new())).await;
    let resp = no_key
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();
    assert!(
        !resp
            .auth_methods
            .iter()
            .any(|m| m.id().to_string() == "agent")
    );
}

#[tokio::test]
async fn authenticate_with_user_key_enables_prompt() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["response"]));
    let agent = make_agent_no_key(Arc::clone(&mock)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("user-test-key")))
        .await
        .unwrap();

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["response"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    agent
        .authenticate(AuthenticateRequest::new("agent"))
        .await
        .unwrap();

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    let agent = make_agent_no_key(Arc::new(MockXaiHttpClient::new())).await;

    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key"))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("XAI_API_KEY missing"),
        "got: {}",
        err.message
    );

    let mut bad_meta = serde_json::Map::new();
    bad_meta.insert("WRONG_KEY".to_string(), serde_json::json!("value"));
    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(bad_meta))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("XAI_API_KEY missing"),
        "got: {}",
        err.message
    );
}

#[tokio::test]
async fn authenticate_empty_key_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(Arc::new(MockXaiHttpClient::new())).await;

    let mut meta = serde_json::Map::new();
    meta.insert("XAI_API_KEY".to_string(), serde_json::json!(""));
    let err = agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(meta))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("must not be empty"),
        "got: {}",
        err.message
    );
}

#[tokio::test]
async fn authenticate_agent_method_without_server_key_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(Arc::new(MockXaiHttpClient::new())).await;

    let err = agent
        .authenticate(AuthenticateRequest::new("agent"))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("no server API key configured"),
        "got: {}",
        err.message
    );
}

#[tokio::test]
async fn authenticate_unknown_method_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let err = agent
        .authenticate(AuthenticateRequest::new("oauth2-mystery"))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("unknown method"),
        "got: {}",
        err.message
    );
}

#[tokio::test]
async fn prompt_fails_with_clear_error_when_session_has_no_api_key() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let err = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap_err();
    assert!(err.message.contains("no API key"), "got: {}", err.message);
}

#[tokio::test]
async fn forked_session_inherits_api_key_from_parent() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["reply"]));
    mock.push_response(text_response(&["reply"]));
    let agent = make_agent_no_key(Arc::clone(&mock)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("user-key")))
        .await
        .unwrap();

    let parent = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let parent_id = parent.session_id.to_string();

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
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["reply"]));
    let agent = make_agent_no_key(Arc::clone(&mock)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("my-user-key-123")))
        .await
        .unwrap();
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls[0].api_key, "my-user-key-123");
}

#[tokio::test]
async fn pending_api_key_consumed_by_first_new_session() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["reply"]));
    let agent = make_agent_no_key(Arc::clone(&mock)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("user-key")))
        .await
        .unwrap();

    let sess1 = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sess2 = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();

    // sess2 has no key (pending key was consumed by sess1).
    let err = agent
        .prompt(PromptRequest::new(
            sess2.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap_err();
    assert!(err.message.contains("no API key"), "got: {}", err.message);

    // sess1 succeeds.
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let resp = agent
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();

    let ev = resp
        .auth_methods
        .iter()
        .find_map(|m| match m {
            AuthMethod::EnvVar(ev) => Some(ev),
            _ => None,
        })
        .expect("expected an EnvVar auth method");

    assert_eq!(ev.vars.len(), 1);
    assert_eq!(ev.vars[0].name, "XAI_API_KEY");
    assert_eq!(ev.link.as_deref(), Some("https://x.ai/api"));
}

// ── session usable after cancel ───────────────────────────────────────────────

#[tokio::test]
async fn session_is_usable_after_cancel() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // First prompt: slow (will be cancelled).
    mock.push_slow_response(XaiEvent::TextDelta {
        text: "partial".to_string(),
    });
    // Second prompt: normal.
    mock.push_response(text_response_with_id("second reply", "resp_2"));
    let agent = std::sync::Arc::new(make_agent(Arc::clone(&mock)).await);

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

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
    assert!(
        agent.test_session_history(&session_id).await.is_empty(),
        "canceled turn must not update history"
    );

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
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["reply"]));
    let agent = make_agent_no_key(Arc::clone(&mock)).await;

    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("key-first")))
        .await
        .unwrap();
    agent
        .authenticate(AuthenticateRequest::new("xai-api-key").meta(api_key_meta("key-second")))
        .await
        .unwrap();

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls[0].api_key, "key-second");
}

// ── multi-block prompt content ────────────────────────────────────────────────

#[tokio::test]
async fn prompt_multi_block_content_is_joined_with_newline() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let content = vec![
        ContentBlock::Text(TextContent::new("line one")),
        ContentBlock::Text(TextContent::new("line two")),
    ];
    agent
        .prompt(PromptRequest::new(sess.session_id.to_string(), content))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let user_msg = calls[0].input.last().expect("at least one input item");
    assert_eq!(user_msg.role, "user");
    assert_eq!(user_msg.content, "line one\nline two");
}

// ── set_session_model affects HTTP request ────────────────────────────────────

#[tokio::test]
async fn set_session_model_is_used_in_http_request_to_xai() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(
            session_id.clone(),
            "grok-3-mini",
        ))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            session_id,
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls[0].model, "grok-3-mini");
}

// ── load_session reflects updated model ──────────────────────────────────────

#[tokio::test]
async fn load_session_reflects_model_after_set_session_model() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(
            session_id.clone(),
            "grok-3-mini",
        ))
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
    let mock = Arc::new(MockXaiHttpClient::new());
    // 3 prompts: 1 shared + 2 diverging
    for _ in 0..3 {
        mock.push_response(text_response(&["reply"]));
    }
    let agent = make_agent(Arc::clone(&mock)).await;

    let src = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let src_id = src.session_id.to_string();
    agent
        .prompt(PromptRequest::new(
            src_id.clone(),
            vec![ContentBlock::Text(TextContent::new("shared"))],
        ))
        .await
        .unwrap();

    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

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

    assert_eq!(src_history.len(), 4);
    assert_eq!(fork_history.len(), 4);

    assert_eq!(src_history[2].content_str(), "parent-only");
    assert_eq!(fork_history[2].content_str(), "fork-only");

    assert!(src_history.iter().all(|m| m.content_str() != "fork-only"));
    assert!(
        fork_history
            .iter()
            .all(|m| m.content_str() != "parent-only")
    );
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
    let agent = XaiAgent::new_in_memory(
        MockSessionNotifier::new(),
        "grok-3",
        "fake-key",
        Arc::new(MockXaiHttpClient::new()),
    );
    assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(300));
}

#[tokio::test]
async fn authenticate_non_string_key_in_meta_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_no_key(Arc::new(MockXaiHttpClient::new())).await;

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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .set_session_mode(agent_client_protocol::SetSessionModeRequest::new(
            session_id,
            "nonexistent-mode",
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown mode"), "got: {err}");
}

#[tokio::test]
async fn set_session_mode_default_succeeds() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_mode(agent_client_protocol::SetSessionModeRequest::new(
            session_id, "default",
        ))
        .await
        .expect("set_session_mode('default') should succeed");
}

#[tokio::test]
async fn set_session_mode_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let err = agent
        .set_session_mode(agent_client_protocol::SetSessionModeRequest::new(
            "no-such-session",
            "default",
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "got: {err}");
}

#[tokio::test]
async fn set_session_config_option_unknown_id_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    let err = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            "no-such-session",
            "unknown_option",
            "value",
        ))
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"), "got: {err:?}");
}

#[tokio::test]
async fn xai_prompt_timeout_secs_zero_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("XAI_BASE_URL");
        std::env::remove_var("XAI_MODELS");
        std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "0");
    }
    let agent = XaiAgent::new_in_memory(
        MockSessionNotifier::new(),
        "grok-3",
        "fake-key",
        Arc::new(MockXaiHttpClient::new()),
    );
    assert_eq!(agent.test_prompt_timeout(), Duration::from_secs(300));
}

#[tokio::test]
async fn xai_models_all_malformed_falls_back_to_defaults() {
    let _guard = env_lock().lock().unwrap();
    let agent =
        make_agent_with_models(Arc::new(MockXaiHttpClient::new()), "nocolon,alsomalformed").await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3"))
        .await
        .unwrap();
    agent
        .set_session_model(SetSessionModelRequest::new(
            session_id.clone(),
            "grok-3-mini",
        ))
        .await
        .unwrap();
}

// ── system prompt / usage / tool calls ───────────────────────────────────────

#[tokio::test]
async fn system_prompt_injected_as_first_message_in_request() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent =
        make_agent_with_system_prompt(Arc::clone(&mock), "You are a helpful assistant.").await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let input = &calls[0].input;

    assert_eq!(input.len(), 2, "system + user");
    assert_eq!(input[0].role, "system");
    assert_eq!(input[0].content, "You are a helpful assistant.");
    assert_eq!(input[1].role, "user");
    assert_eq!(input[1].content, "hello");
}

#[tokio::test]
async fn system_prompt_absent_when_env_var_not_set() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let input = &calls[0].input;

    assert_eq!(input.len(), 1, "only user message — no system message");
    assert_eq!(input[0].role, "user");
}

#[tokio::test]
async fn system_prompt_present_in_subsequent_turns() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response_with_id("reply1", "resp_0"));
    mock.push_response(text_response(&["reply2"]));
    let agent = make_agent_with_system_prompt(Arc::clone(&mock), "Be concise.").await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("turn 1"))],
        ))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("turn 2"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    // Turn 1: system + user
    let input1 = &calls[0].input;
    assert_eq!(input1.len(), 2, "system + user");
    assert_eq!(input1[0].role, "system");
    assert_eq!(input1[0].content, "Be concise.");

    // Turn 2: stateful — only the new user message (system already in xAI context).
    let input2 = &calls[1].input;
    assert_eq!(input2.len(), 1, "only new user message on turn 2");
    assert_eq!(input2[0].role, "user");
    assert_eq!(input2[0].content, "turn 2");
    assert!(
        calls[1].previous_response_id.is_some(),
        "turn 2 must have previous_response_id"
    );
}

#[tokio::test]
async fn usage_chunk_is_handled_and_prompt_completes() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_with_usage("Hello"));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("hi"))],
        ))
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
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(function_call_response(
        "call_test",
        "web_search",
        "{\"query\":\"rust\"}",
    ));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("search"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "search");
}

// ── tool config options ───────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_exposes_all_tool_config_options_as_off() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let resp = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();

    let opts = resp
        .config_options
        .expect("config_options should be present");
    assert_eq!(opts.len(), 2, "expected 2 tool toggles: {opts:?}");
    let ids: Vec<String> = opts.iter().map(|o| o.id.to_string()).collect();
    assert!(ids.contains(&"web_search".to_string()));
    assert!(ids.contains(&"x_search".to_string()));
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .load_session(LoadSessionRequest::new(sid.clone(), "/tmp"))
        .await
        .unwrap();

    let opts = resp
        .config_options
        .expect("config_options should be present");
    assert_eq!(opts.len(), 2, "expected 2 tool toggle options");
}

#[tokio::test]
async fn enable_web_search_persists_and_is_reflected_in_response() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();

    assert_eq!(resp.config_options.len(), 2);
    let ws = resp
        .config_options
        .iter()
        .find(|o| o.id.to_string() == "web_search")
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
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let err = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "turbo",
        ))
        .await
        .unwrap_err();

    assert!(
        err.message.contains("turbo"),
        "error should mention the bad value: {err:?}"
    );
}

#[tokio::test]
async fn enabled_tools_are_sent_in_request_tools_array() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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

    let calls = mock.calls.lock().unwrap();
    assert!(
        calls[0].tools.iter().any(|t| t == "web_search"),
        "web_search must be in tools: {:?}",
        calls[0].tools
    );
}

#[tokio::test]
async fn no_tools_omits_tools_field_from_request() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("plain query"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert!(
        calls[0].tools.is_empty(),
        "tools must be empty when no tools are enabled"
    );
}

#[tokio::test]
async fn fork_session_inherits_enabled_tools() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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

    let opts = fork
        .config_options
        .expect("config_options should be present in fork");
    let ws = opts
        .iter()
        .find(|o| o.id.to_string() == "web_search")
        .expect("web_search option must be present");
    let current = match &ws.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on", "forked session should inherit web_search=on");
}

// ── XAI_MAX_HISTORY_MESSAGES env var ─────────────────────────────────────────

#[tokio::test]
async fn xai_max_history_messages_default_is_20() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    assert_eq!(agent.test_max_history_messages(), 20);
}

#[tokio::test]
async fn xai_max_history_messages_zero_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_with_max_history(Arc::new(MockXaiHttpClient::new()), 0).await;
    assert_eq!(agent.test_max_history_messages(), 20);
}

#[tokio::test]
async fn xai_max_history_messages_custom_value_accepted() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent_with_max_history(Arc::new(MockXaiHttpClient::new()), 10).await;
    assert_eq!(agent.test_max_history_messages(), 10);
}

// ── XAI_MAX_TURNS env var ─────────────────────────────────────────────────────

#[tokio::test]
async fn xai_max_turns_zero_means_server_default() {
    // XAI_MAX_TURNS=0 must produce None — the field is omitted from the request
    // so xAI uses its own server-side default rather than our app default of 10.
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_BASE_URL");
        std::env::set_var("XAI_MAX_TURNS", "0");
    }
    let agent = XaiAgent::new_in_memory(
        MockSessionNotifier::new(), "grok-3", "fake-key",
        Arc::new(MockXaiHttpClient::new()),
    );
    assert_eq!(agent.test_max_turns(), None, "XAI_MAX_TURNS=0 must produce None (server default)");
}

#[tokio::test]
async fn xai_max_turns_default_is_ten() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    assert_eq!(agent.test_max_turns(), Some(10));
}

#[tokio::test]
async fn xai_max_turns_custom_value_accepted() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("XAI_MODELS");
        std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS");
        std::env::remove_var("XAI_SYSTEM_PROMPT");
        std::env::remove_var("XAI_MAX_HISTORY_MESSAGES");
        std::env::remove_var("XAI_BASE_URL");
        std::env::set_var("XAI_MAX_TURNS", "5");
    }
    let agent = XaiAgent::new_in_memory(
        MockSessionNotifier::new(), "grok-3", "fake-key",
        Arc::new(MockXaiHttpClient::new()),
    );
    assert_eq!(agent.test_max_turns(), Some(5));
}

#[tokio::test]
async fn history_is_truncated_after_exceeding_max() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    for _ in 0..3 {
        mock.push_response(text_response(&["reply"]));
    }
    let agent = make_agent_with_max_history(Arc::clone(&mock), 4).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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

    let history = agent.test_session_history(&sid).await;
    assert_eq!(
        history.len(),
        4,
        "expected 4 messages, got {}: {history:?}",
        history.len()
    );
    assert_eq!(
        history[0].content_str(),
        "turn 2",
        "oldest pair should have been dropped"
    );
    assert_eq!(history[2].content_str(), "turn 3");
}

#[tokio::test]
async fn history_truncation_drops_oldest_pair_first() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    for _ in 0..3 {
        mock.push_response(text_response(&["reply"]));
    }
    let agent = make_agent_with_max_history(Arc::clone(&mock), 2).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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

    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content_str(), "turn 3");
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

// ── coverage gaps ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn x_search_tool_enabled_is_included_in_tools_array() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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

    let calls = mock.calls.lock().unwrap();
    assert!(
        calls[0].tools.iter().any(|t| t == "x_search"),
        "x_search must be in tools: {:?}",
        calls[0].tools
    );
}

#[tokio::test]
async fn set_session_config_option_unknown_session_returns_error() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

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

#[tokio::test]
async fn history_at_exactly_the_limit_is_not_truncated() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    for _ in 0..2 {
        mock.push_response(text_response(&["reply"]));
    }
    let agent = make_agent_with_max_history(Arc::clone(&mock), 4).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
    assert_eq!(history.len(), 4, "no truncation at limit: {history:?}");
    assert_eq!(history[0].content_str(), "turn 1");
    assert_eq!(history[2].content_str(), "turn 2");
}

#[tokio::test]
async fn history_truncation_rounds_up_to_even_for_odd_max() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    for _ in 0..2 {
        mock.push_response(text_response(&["reply"]));
    }
    let agent = make_agent_with_max_history(Arc::clone(&mock), 3).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
        "odd max=3: oldest pair dropped, 2 remain: {history:?}"
    );
    assert_eq!(history[0].content_str(), "turn 2");
}

#[tokio::test]
async fn multiple_tool_calls_in_one_turn_do_not_panic() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::ResponseId {
            id: "resp_tools".to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "call_a".to_string(),
            name: "web_search".to_string(),
            arguments: "{\"q\":\"rust\"}".to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "call_b".to_string(),
            name: "get_weather".to_string(),
            arguments: "{\"city\":\"London\"}".to_string(),
        },
        XaiEvent::Done,
    ]);
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("use two tools"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
}

#[tokio::test]
async fn set_session_config_option_unknown_id_returns_current_state() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "on",
        ))
        .await
        .unwrap();

    let resp = agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "unknown_option",
            "value",
        ))
        .await
        .unwrap();

    assert_eq!(resp.config_options.len(), 2);
    let ws = resp
        .config_options
        .iter()
        .find(|o| o.id.to_string() == "web_search")
        .expect("web_search option must be present");
    let current = match &ws.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on");
}

#[tokio::test]
async fn load_session_reflects_updated_tool_state() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    agent
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "x_search",
            "on",
        ))
        .await
        .unwrap();

    let loaded = agent
        .load_session(LoadSessionRequest::new(sid.clone(), "/tmp"))
        .await
        .unwrap();
    let opts = loaded
        .config_options
        .expect("config_options should be present");
    assert_eq!(opts.len(), 2);
    let xs = opts
        .iter()
        .find(|o| o.id.to_string() == "x_search")
        .expect("x_search option must be present");
    let current = match &xs.kind {
        agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
        _ => panic!("expected Select kind"),
    };
    assert_eq!(current, "on");
}

#[tokio::test]
async fn disabling_tool_removes_it_from_request() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            sid.clone(),
            "web_search",
            "off",
        ))
        .await
        .unwrap();

    agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("query"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert!(
        calls[0].tools.is_empty(),
        "tools must be absent after disabling all tools"
    );
}

#[tokio::test]
async fn close_session_cancels_in_flight_prompt() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_slow_response(XaiEvent::TextDelta {
        text: "partial".to_string(),
    });
    let agent = std::sync::Arc::new(make_agent(Arc::clone(&mock)).await);

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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
            agent2
                .close_session(CloseSessionRequest::new(sid))
                .await
                .unwrap();
        }
    );

    assert_eq!(prompt_result.unwrap().stop_reason, StopReason::Cancelled);

    let list = agent
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();
    assert!(
        list.sessions.is_empty(),
        "session must be deleted by close_session"
    );
}

#[tokio::test]
async fn close_session_unknown_session_is_noop() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;
    agent
        .close_session(CloseSessionRequest::new("no-such-session"))
        .await
        .expect("close_session on unknown id should succeed silently");
}

#[tokio::test]
async fn prompt_timeout_with_no_text_does_not_update_history() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // Yield an empty text delta (effectively no visible text), then block.
    mock.push_slow_response(XaiEvent::TextDelta {
        text: "".to_string(),
    });
    let agent = make_agent_with_timeout(Arc::clone(&mock), 1).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::Cancelled);

    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content_str(), "hello");
}

#[tokio::test]
async fn fork_session_inherits_system_prompt() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // Parent prompt + fork prompt
    mock.push_response(text_response_with_id("parent reply", "resp_p"));
    mock.push_response(text_response(&["fork reply"]));
    let agent = make_agent_with_system_prompt(Arc::clone(&mock), "Be concise.").await;

    let src = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let src_id = src.session_id.to_string();
    agent
        .prompt(PromptRequest::new(
            src_id.clone(),
            vec![ContentBlock::Text(TextContent::new("parent question"))],
        ))
        .await
        .unwrap();

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

    // Fork clears last_response_id, so fork's first prompt sends full history.
    let calls = mock.calls.lock().unwrap();
    let fork_input = &calls[1].input;
    assert_eq!(
        fork_input[0].role, "system",
        "first input item of fork must be system: {fork_input:?}"
    );
    assert_eq!(fork_input[0].content, "Be concise.");
}

// ── ContentBlock::ResourceLink and ContentBlock::Resource ─────────────────────

#[tokio::test]
async fn resource_link_is_included_as_text_in_request() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::ResourceLink(ResourceLink::new(
                "README",
                "file:///project/README.md",
            ))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let content = calls[0].input.last().unwrap().content.as_str();
    assert!(
        content.contains("README") && content.contains("file:///project/README.md"),
        "ResourceLink must be forwarded as text: {content:?}"
    );
}

#[tokio::test]
async fn embedded_text_resource_is_included_as_text_in_request() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                    "fn main() {}",
                    "file:///src/main.rs",
                )),
            ))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let content = calls[0].input.last().unwrap().content.as_str();
    assert_eq!(content, "fn main() {}");
}

#[tokio::test]
async fn mixed_text_and_resource_link_blocks_are_joined() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
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

    let calls = mock.calls.lock().unwrap();
    let content = calls[0].input.last().unwrap().content.as_str();
    assert!(
        content.starts_with("Please review:"),
        "text block must come first: {content:?}"
    );
    assert!(
        content.contains("main.rs"),
        "resource link must be included: {content:?}"
    );
}

// ── response.error and server-cancelled events ────────────────────────────────

#[tokio::test]
async fn response_error_event_surfaces_as_acp_error() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(server_failed_response("Request blocked by content policy"));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("test"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::InternalError.into()
    );
    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "history must be empty after response.error: {history:?}"
    );
}

#[tokio::test]
async fn xai_server_cancelled_surfaces_as_acp_error() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(server_cancelled_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("test"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::InternalError.into()
    );
    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "history must be empty after server-side cancel: {history:?}"
    );
}

#[tokio::test]
async fn api_401_error_surfaces_as_acp_error_without_retry() {
    // A 4xx error must NOT trigger the stale-ID retry. Verify by pre-loading
    // exactly one mock response — if the agent retried, it would panic on the
    // second call (no response queued).
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(client_error_response());
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("ping"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::InternalError.into()
    );
    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "history must be empty after 401 error: {history:?}"
    );
}

// ── Incomplete continuation ───────────────────────────────────────────────────

#[tokio::test]
async fn incomplete_max_output_tokens_continues_and_assembles_text() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // First request: partial text, then incomplete.
    mock.push_response(incomplete_max_output_tokens("Hello ", "resp_part"));
    // Second request (continuation): rest of the text, then completed.
    mock.push_response(continuation_response("world", "resp_cont"));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("say hello world"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected exactly 2 HTTP requests");

    // First request: user message, no previous_response_id.
    assert!(
        calls[0].input.iter().any(|i| i.role == "user"),
        "first request must carry the user message"
    );
    assert!(
        calls[0].previous_response_id.is_none(),
        "first request must not have previous_response_id"
    );

    // Continuation: empty input + previous_response_id from the incomplete response.
    assert!(
        calls[1].input.is_empty(),
        "max_output_tokens continuation must send empty input"
    );
    assert_eq!(
        calls[1].previous_response_id.as_deref(),
        Some("resp_part"),
        "continuation must reference the incomplete response's ID"
    );

    drop(calls);

    // History: user turn + assembled assistant text from both streams.
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1].content_str(),
        "Hello world",
        "assembled text must concatenate both chunks"
    );
}

// ── tool-call → Failed on stream error ───────────────────────────────────────

#[tokio::test]
async fn tool_call_then_response_failed_compensates_history() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::ResponseId {
            id: "resp_toolerr".to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "call_1".to_string(),
            name: "web_search".to_string(),
            arguments: "{}".to_string(),
        },
        XaiEvent::Finished {
            reason: FinishReason::Failed,
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]);
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let session_id = sess.session_id.to_string();

    let err = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("search for something"))],
        ))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::InternalError.into()
    );
    let history = agent.test_session_history(&session_id).await;
    assert!(
        history.is_empty(),
        "history must be empty after response.failed: {history:?}"
    );
}

// ── search call lifecycle events ──────────────────────────────────────────────

#[tokio::test]
async fn web_search_call_completed_advances_tool_status_before_done() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::ResponseId {
            id: "resp_ws".to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "call_1".to_string(),
            name: "web_search".to_string(),
            arguments: "{\"query\":\"rust async\"}".to_string(),
        },
        XaiEvent::ServerToolCompleted {
            name: "web_search".to_string(),
        },
        XaiEvent::TextDelta {
            text: "Rust async is great".to_string(),
        },
        XaiEvent::Finished {
            reason: FinishReason::Completed,
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]);
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new(
                "search for rust async",
            ))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content_str(), "Rust async is great");
}

#[tokio::test]
async fn x_search_call_completed_advances_tool_status_before_done() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::ResponseId {
            id: "resp_xs".to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "call_2".to_string(),
            name: "x_search".to_string(),
            arguments: "{\"query\":\"rust 2024\"}".to_string(),
        },
        XaiEvent::ServerToolCompleted {
            name: "x_search".to_string(),
        },
        XaiEvent::TextDelta {
            text: "X search result".to_string(),
        },
        XaiEvent::Finished {
            reason: FinishReason::Completed,
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]);
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new(
                "search x for rust 2024",
            ))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content_str(), "X search result");
}

#[tokio::test]
async fn search_call_completed_without_prior_function_call_is_noop() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::ServerToolCompleted {
            name: "web_search".to_string(),
        },
        XaiEvent::TextDelta {
            text: "answer".to_string(),
        },
        XaiEvent::Finished {
            reason: FinishReason::Completed,
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]);
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("question"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content_str(), "answer");
}

// ── stale-ID retry ────────────────────────────────────────────────────────────

#[tokio::test]
async fn stale_id_retry_on_non_4xx_error_succeeds_transparently() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // Turn 1: succeeds, stores last_response_id = "resp_turn1".
    mock.push_response(text_response_with_id("first reply", "resp_turn1"));
    // Turn 2, first attempt: 500 error (triggers stale-ID retry).
    mock.push_response(error_response()); // contains "500" not "4xx"
    // Turn 2, retry: succeeds.
    mock.push_response(text_response_with_id("second reply", "resp_turn2"));

    let agent = make_agent(Arc::clone(&mock)).await;
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let r1 = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("first question"))],
        ))
        .await
        .unwrap();
    assert_eq!(r1.stop_reason, StopReason::EndTurn);

    let r2 = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("second question"))],
        ))
        .await
        .unwrap();
    assert_eq!(r2.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 3, "expected 3 HTTP requests total");

    // Request 1 (turn 1): no previous_response_id.
    assert!(
        calls[0].previous_response_id.is_none(),
        "turn 1 must not send previous_response_id"
    );

    // Request 2 (turn 2, first attempt): carries the stale previous_response_id.
    assert_eq!(
        calls[1].previous_response_id.as_deref(),
        Some("resp_turn1"),
        "turn 2 first attempt must send previous_response_id = resp_turn1"
    );

    // Request 3 (turn 2, retry): no previous_response_id, sends full history.
    assert!(
        calls[2].previous_response_id.is_none(),
        "stale-ID retry must not carry previous_response_id"
    );
    assert!(
        calls[2].input.len() >= 2,
        "retry must include full history, got {:?}",
        calls[2].input.iter().map(|i| &i.role).collect::<Vec<_>>()
    );

    drop(calls);

    let history = agent.test_session_history(&sid).await;
    assert_eq!(
        history.len(),
        4,
        "history must have 4 messages after 2 turns"
    );
}

// ── MAX_CONTINUATIONS exhaustion ──────────────────────────────────────────────

#[tokio::test]
async fn max_continuations_exhausted_returns_cancelled_with_partial_text() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // 6 incomplete responses (MAX_CONTINUATIONS = 5, guard fires on 6th).
    for i in 0u32..6 {
        let resp_id = format!("resp_part{i}");
        let chunk = format!("chunk{i} ");
        mock.push_response(incomplete_max_output_tokens(&chunk, &resp_id));
    }
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("keep going"))],
        ))
        .await
        .unwrap();

    assert_eq!(
        resp.stop_reason,
        StopReason::Cancelled,
        "exhausted continuations must return Cancelled"
    );

    let history = agent.test_session_history(&sid).await;
    assert_eq!(history.len(), 2);
    let saved_text = history[1].content_str();
    assert!(
        saved_text.contains("chunk0"),
        "partial text must include chunks: {saved_text:?}"
    );
    assert!(
        saved_text.contains("chunk5"),
        "partial text must include last chunk: {saved_text:?}"
    );
}

// ── max_turns continuation input ─────────────────────────────────────────────

#[tokio::test]
async fn incomplete_max_turns_continuation_resends_user_input() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    // First request: incomplete due to max_turns.
    mock.push_response(incomplete_max_turns("resp_mt"));
    // Second request (continuation): final answer.
    mock.push_response(text_response(&["final answer"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let sid = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            sid.clone(),
            vec![ContentBlock::Text(TextContent::new("ask with tools"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected 2 requests");

    // Continuation for max_turns: must re-send the user question (not empty input).
    assert!(
        !calls[1].input.is_empty(),
        "max_turns continuation must not send empty input"
    );
    assert_eq!(
        calls[1].previous_response_id.as_deref(),
        Some("resp_mt"),
        "continuation must reference the incomplete response's ID"
    );
    let user_msg = calls[1].input.iter().find(|i| i.role == "user");
    assert!(
        user_msg.is_some(),
        "max_turns continuation must include the user question"
    );
    assert_eq!(user_msg.unwrap().content, "ask with tools");
}
