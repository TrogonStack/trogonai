//! Integration tests for `XaiAgent`.
//!
//! All tests use an in-process `MockXaiHttpClient` — no TCP servers are needed
//! for the xAI HTTP layer. A minimal fake NATS TCP stub is still used to obtain
//! a real `async_nats::Client` (fire-and-forget PUBs are silently dropped).
#![allow(clippy::await_holding_lock)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::arc_with_non_send_sync)]
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
    let user_msgs: Vec<_> = calls[0].input.iter().filter(|i| i.role() == Some("user")).collect();
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
    assert_eq!(calls[0].input[0].role().unwrap(), "user");
    assert_eq!(calls[0].input[0].content().unwrap(), "first question");
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
    assert_eq!(calls[1].input[0].role().unwrap(), "user");
    assert_eq!(calls[1].input[0].content().unwrap(), "second question");
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
    assert_eq!(user_msg.role().unwrap(), "user");
    assert_eq!(user_msg.content().unwrap(), "line one\nline two");
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
    assert_eq!(input[0].role().unwrap(), "system");
    assert_eq!(input[0].content().unwrap(), "You are a helpful assistant.");
    assert_eq!(input[1].role().unwrap(), "user");
    assert_eq!(input[1].content().unwrap(), "hello");
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
    assert_eq!(input[0].role().unwrap(), "user");
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
    assert_eq!(input1[0].role().unwrap(), "system");
    assert_eq!(input1[0].content().unwrap(), "Be concise.");

    // Turn 2: stateful — only the new user message (system already in xAI context).
    let input2 = &calls[1].input;
    assert_eq!(input2.len(), 1, "only new user message on turn 2");
    assert_eq!(input2[0].role().unwrap(), "user");
    assert_eq!(input2[0].content().unwrap(), "turn 2");
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
        calls[0].tools.iter().any(|t| t.name() == "web_search"),
        "web_search must be in tools: {:?}",
        calls[0].tools.iter().map(|t| t.name()).collect::<Vec<_>>()
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
        MockSessionNotifier::new(),
        "grok-3",
        "fake-key",
        Arc::new(MockXaiHttpClient::new()),
    );
    assert_eq!(
        agent.test_max_turns(),
        None,
        "XAI_MAX_TURNS=0 must produce None (server default)"
    );
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
        MockSessionNotifier::new(),
        "grok-3",
        "fake-key",
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
        calls[0].tools.iter().any(|t| t.name() == "x_search"),
        "x_search must be in tools: {:?}",
        calls[0].tools.iter().map(|t| t.name()).collect::<Vec<_>>()
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
        fork_input[0].role().unwrap(), "system",
        "first input item of fork must be system: {fork_input:?}"
    );
    assert_eq!(fork_input[0].content().unwrap(), "Be concise.");
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
    let content = calls[0].input.last().unwrap().content().unwrap_or("");
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
    let content = calls[0].input.last().unwrap().content().unwrap_or("");
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
    let content = calls[0].input.last().unwrap().content().unwrap_or("");
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
        calls[0].input.iter().any(|i| i.role() == Some("user")),
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
        calls[2].input.iter().map(|i| i.role().unwrap_or("")).collect::<Vec<_>>()
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
    let user_msg = calls[1].input.iter().find(|i| i.role() == Some("user"));
    assert!(
        user_msg.is_some(),
        "max_turns continuation must include the user question"
    );
    assert_eq!(user_msg.unwrap().content().unwrap(), "ask with tools");
}

// ── Bash tool round-trip via wasm-runtime ─────────────────────────────────────

/// Verifies the full bash tool execution path:
/// 1. Mock HTTP returns a `FunctionCall { name: "bash" }` in round 1.
/// 2. Agent routes the call through NATS to a fake wasm-runtime terminal responder.
/// 3. Agent sends a second HTTP request with the `function_call_output` and the
///    correct `previous_response_id`.
#[tokio::test]
async fn bash_tool_call_round_trips_through_wasm_runtime() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_xai_runner::InputItem;

    // ── Start NATS JetStream ──────────────────────────────────────────────
    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());

    // ── Register fake wasm-runtime in the registry ────────────────────────
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    };
    registry.register(&cap).await.unwrap();

    // ── Spawn terminal responder ──────────────────────────────────────────
    // Handles the four sequential NATS requests the agent issues per bash call.
    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => continue,
            };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                let resp = CreateTerminalResponse::new(TerminalId::new("tid-1"));
                serde_json::to_vec(&resp).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                let resp = WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                );
                serde_json::to_vec(&resp).unwrap()
            } else if subject.ends_with(".output") {
                let resp = TerminalOutputResponse::new("hello\n".to_string(), false);
                serde_json::to_vec(&resp).unwrap()
            } else {
                vec![]
            };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── Build agent with execution backend ───────────────────────────────
    let mock = Arc::new(MockXaiHttpClient::new());
    let _env_guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // Round 1: model requests a bash call
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo hello"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    // Round 2: model produces the final answer after seeing the tool output
    mock.push_response(vec![
        XaiEvent::TextDelta { text: "All done!".to_string() },
        XaiEvent::ResponseId { id: "resp_2".to_string() },
        XaiEvent::Finished { reason: FinishReason::Completed, incomplete_reason: None },
        XaiEvent::Done,
    ]);

    // ── Run prompt ────────────────────────────────────────────────────────
    let sess = agent
        .new_session(NewSessionRequest::new("/tmp"))
        .await
        .unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run a command"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected 2 HTTP calls: bash round + final answer");

    // Second call must carry the first response's id as context
    assert_eq!(
        calls[1].previous_response_id.as_deref(),
        Some("resp_bash"),
        "round 2 must reference resp_bash as previous_response_id"
    );

    // Second call's input must contain the bash function_call_output
    let fo = calls[1]
        .input
        .iter()
        .find(|i| matches!(i, InputItem::FunctionCallOutput { call_id, .. } if call_id == "cid1"));
    assert!(fo.is_some(), "round 2 input must contain function_call_output for cid1");
    if let Some(InputItem::FunctionCallOutput { output, .. }) = fo {
        assert!(
            output.contains("hello"),
            "bash output must be forwarded to model; got {output:?}"
        );
    }
}

// ── bash feature: no execution backend ───────────────────────────────────────

/// Without an execution backend, the agent must not advertise `bash` in the
/// tools array sent to xAI.
#[tokio::test]
async fn no_execution_backend_means_bash_not_in_tools() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    let agent = make_agent(mock.clone()).await; // no with_execution_backend

    mock.push_response(text_response_with_id("ok", "resp_1"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert!(
        !calls[0].tools.iter().any(|t| t.name() == "bash"),
        "bash must not appear in tools without an execution backend"
    );
}

// ── bash feature: non-bash function call ─────────────────────────────────────

/// When the model returns a function call for a non-bash tool, the agent must
/// send a `ToolCallUpdate(Completed)` notification and exit cleanly.
#[tokio::test]
async fn non_bash_function_call_gets_completed_notification() {
    use agent_client_protocol::{SessionUpdate, ToolCallStatus};

    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    let agent = make_agent(mock.clone()).await;

    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_1".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "unknown_tool".to_string(),
            arguments: "{}".to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("do something"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let notifs = agent.test_notifier().notifications.lock().unwrap();
    let completed = notifs.iter().find(|n| {
        if let SessionUpdate::ToolCallUpdate(u) = &n.update {
            u.tool_call_id.to_string() == "cid1"
                && u.fields.status == Some(ToolCallStatus::Completed)
        } else {
            false
        }
    });
    assert!(
        completed.is_some(),
        "expected ToolCallUpdate(Completed) for cid1; got: {notifs:?}"
    );
}

// ── bash feature: pending calls cleared on Incomplete ────────────────────────

/// If the stream contains both a function call and `FinishReason::Incomplete`
/// in the same response, the agent must clear pending tool calls and issue a
/// plain continuation — no `function_call_output` in the follow-up request.
#[tokio::test]
async fn pending_tool_calls_cleared_when_incomplete_response_contains_function_call() {
    use trogon_xai_runner::InputItem;

    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    let agent = make_agent(mock.clone()).await;

    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_part".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid_discard".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo hi"}"#.to_string(),
        },
        XaiEvent::Finished {
            reason: FinishReason::Incomplete,
            incomplete_reason: Some("max_turns".to_string()),
        },
        XaiEvent::Done,
    ]);
    mock.push_response(continuation_response("done", "resp_cont"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run bash"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected a continuation HTTP call");

    let has_fco = calls[1]
        .input
        .iter()
        .any(|i| matches!(i, InputItem::FunctionCallOutput { .. }));
    assert!(
        !has_fco,
        "continuation round must not forward pending function_call_outputs; input: {:?}",
        calls[1].input
    );
}

// ── bash feature: NATS-backed tests ──────────────────────────────────────────

/// The agent must send `ToolCall(Pending)` → `ToolCall(InProgress)` →
/// `ToolCallUpdate(Completed, raw_output=<bash output>)` for each bash call.
#[tokio::test]
async fn bash_notification_sequence_pending_inprogress_completed_with_raw_output() {
    use agent_client_protocol::{
        CreateTerminalResponse, SessionUpdate, TerminalExitStatus, TerminalId,
        TerminalOutputResponse, ToolCallStatus, ToolKind, WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    };
    registry.register(&cap).await.unwrap();

    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new("tid-1"))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new("hello\n".to_string(), false)).unwrap()
            } else {
                vec![]
            };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo hello"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    mock.push_response(text_response_with_id("All done!", "resp_2"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run echo"))],
        ))
        .await
        .unwrap();

    let notifs = agent.test_notifier().notifications.lock().unwrap();

    // 1. Pending: emitted when FunctionCall SSE event is received
    let pending = notifs.iter().find(|n| {
        if let SessionUpdate::ToolCall(tc) = &n.update {
            tc.tool_call_id.to_string() == "cid1"
                && tc.status == ToolCallStatus::Pending
                && tc.kind == ToolKind::Execute
        } else {
            false
        }
    });
    assert!(pending.is_some(), "expected ToolCall(Pending, Execute) for cid1; got: {notifs:?}");

    // 2. InProgress: emitted just before NATS execution
    let in_progress = notifs.iter().find(|n| {
        if let SessionUpdate::ToolCall(tc) = &n.update {
            tc.tool_call_id.to_string() == "cid1" && tc.status == ToolCallStatus::InProgress
        } else {
            false
        }
    });
    assert!(
        in_progress.is_some(),
        "expected ToolCall(InProgress) for cid1; got: {notifs:?}"
    );

    // 3. Completed: emitted after NATS returns the output, with raw_output set
    let completed = notifs.iter().find(|n| {
        if let SessionUpdate::ToolCallUpdate(u) = &n.update {
            u.tool_call_id.to_string() == "cid1"
                && u.fields.status == Some(ToolCallStatus::Completed)
                && u.fields.raw_output.is_some()
        } else {
            false
        }
    });
    assert!(
        completed.is_some(),
        "expected ToolCallUpdate(Completed, raw_output) for cid1; got: {notifs:?}"
    );

    // raw_output must contain the bash output string
    if let Some(n) = completed {
        if let SessionUpdate::ToolCallUpdate(u) = &n.update {
            let raw = u.fields.raw_output.as_ref().unwrap();
            assert!(
                raw.as_str().unwrap_or("").contains("hello"),
                "raw_output must contain bash stdout; got: {raw:?}"
            );
        }
    }
}

/// After `MAX_TOOL_ROUNDS` (10) bash executions, the next bash call must cause
/// the agent to return `StopReason::Cancelled` without executing the tool.
#[tokio::test]
async fn max_tool_rounds_cap_returns_cancelled() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    };
    registry.register(&cap).await.unwrap();

    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new("tid-1"))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new("ok\n".to_string(), false)).unwrap()
            } else {
                vec![]
            };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // 11 bash responses: rounds 1-10 execute successfully, round 11 hits the cap.
    for _ in 0..11 {
        mock.push_response(vec![
            XaiEvent::ResponseId { id: "resp_bash".to_string() },
            XaiEvent::FunctionCall {
                call_id: "cid".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"echo ok"}"#.to_string(),
            },
            XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
            XaiEvent::Done,
        ]);
    }

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("loop bash"))],
        ))
        .await
        .unwrap();

    assert_eq!(
        resp.stop_reason,
        StopReason::Cancelled,
        "hitting MAX_TOOL_ROUNDS must return Cancelled"
    );
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 11, "expected exactly 11 HTTP calls (10 bash + 1 cap hit)");
}

/// When the NATS terminal create handler returns invalid JSON, `execute_bash_via_nats`
/// must return an error string that the agent forwards as the tool result so the
/// model can react to the failure.
#[tokio::test]
async fn bash_nats_error_forwarded_as_tool_result_to_model() {
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_xai_runner::InputItem;

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    };
    registry.register(&cap).await.unwrap();

    // Responder returns invalid JSON for `create` — triggers a parse error.
    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply.clone() {
                let subject: &str = msg.subject.as_ref();
                if subject.ends_with(".create") {
                    let _ = nats_srv.publish(reply, b"not-valid-json".as_ref().into()).await;
                }
                // Other subjects receive no reply (never reached since create fails first).
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo hi"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    mock.push_response(text_response_with_id("noted", "resp_2"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("try bash"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected bash round + follow-up");

    let fco = calls[1]
        .input
        .iter()
        .find(|i| matches!(i, InputItem::FunctionCallOutput { call_id, .. } if call_id == "cid1"));
    assert!(fco.is_some(), "round 2 must contain function_call_output for cid1");
    if let Some(InputItem::FunctionCallOutput { output, .. }) = fco {
        assert!(
            output.starts_with("error:"),
            "bash NATS parse error must be forwarded as 'error: ...' string; got: {output:?}"
        );
    }
}

/// After bash rounds complete, if the final model response carries no ResponseId
/// the stored `last_response_id` must be cleared to prevent a stale ID from
/// being sent as `previous_response_id` on the next prompt.
#[tokio::test]
async fn last_response_id_cleared_when_bash_ran_but_final_round_has_no_response_id() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    };
    registry.register(&cap).await.unwrap();

    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new("tid-1"))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new("ok\n".to_string(), false)).unwrap()
            } else {
                vec![]
            };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // Seed a pre-existing response ID so we can confirm it gets cleared.
    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();
    agent
        .test_insert_session_with_response_id(&session_id, "/tmp", None, Some("old_id".to_string()))
        .await;

    // Round 1: bash call with a response ID.
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo ok"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    // Round 2: final text reply — no ResponseId event.
    mock.push_response(vec![
        XaiEvent::TextDelta { text: "done".to_string() },
        XaiEvent::Done,
    ]);

    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("run it"))],
        ))
        .await
        .unwrap();

    assert_eq!(
        agent.test_last_response_id(&session_id).await,
        None,
        "last_response_id must be cleared when bash ran but the final round returned no ID"
    );
}

// ── bash feature: remaining gap coverage ─────────────────────────────────────

/// When `execute_bash_via_nats` receives arguments without a `command` field it
/// returns an error string immediately (no NATS calls made). The agent must
/// forward that error string as the `function_call_output` so the model can react.
#[tokio::test]
async fn bash_missing_command_field_returns_error_string_to_model() {
    use async_nats::jetstream;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_xai_runner::InputItem;

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();
    // No terminal responder — execute_bash_via_nats must return before touching NATS.

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"arg": "not_command"}"#.to_string(), // missing "command"
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    mock.push_response(text_response_with_id("noted", "resp_2"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("try bash"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "missing-command error must still yield a second HTTP call");

    let fco = calls[1]
        .input
        .iter()
        .find(|i| matches!(i, InputItem::FunctionCallOutput { call_id, .. } if call_id == "cid1"));
    assert!(fco.is_some(), "round 2 must contain function_call_output for cid1");
    if let Some(InputItem::FunctionCallOutput { output, .. }) = fco {
        assert!(
            output.contains("error"),
            "missing 'command' must produce an error string; got: {output:?}"
        );
    }
}

/// When the model emits `FunctionCall+ToolCalls` but no `ResponseId` in the same
/// stream, the agent cannot safely submit the tool result (it needs a
/// `previous_response_id`). It must skip execution and return normally.
#[tokio::test]
async fn bash_skipped_when_stream_has_no_response_id() {
    use async_nats::jetstream;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // Stream: bash call + ToolCalls but NO ResponseId event.
    mock.push_response(vec![
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo hi"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run bash"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "bash must be silently skipped when ResponseId is absent — no second HTTP call"
    );
}

/// Multiple bash calls in the same round must all be executed sequentially and
/// each must produce its own `function_call_output` in the follow-up request.
#[tokio::test]
async fn multiple_bash_calls_in_one_round_all_forwarded_as_function_call_outputs() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_xai_runner::InputItem;

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();

    // Terminal responder: reply "out-<create-count>" so each call returns distinct output.
    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        let mut n: u32 = 0;
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                n += 1;
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new(
                    format!("tid-{n}"),
                ))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new(
                    format!("out-{n}"),
                    false,
                )).unwrap()
            } else {
                vec![]
            };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // Round 1: two bash calls in the same response.
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo first"}"#.to_string(),
        },
        XaiEvent::FunctionCall {
            call_id: "cid2".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo second"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    mock.push_response(text_response_with_id("all done", "resp_2"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run two commands"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "expected bash round + final answer");

    let fcos: Vec<_> = calls[1]
        .input
        .iter()
        .filter(|i| matches!(i, InputItem::FunctionCallOutput { .. }))
        .collect();
    assert_eq!(fcos.len(), 2, "both bash calls must produce function_call_output items; got: {:?}", fcos);

    let ids: Vec<&str> = fcos
        .iter()
        .filter_map(|i| {
            if let InputItem::FunctionCallOutput { call_id, .. } = i {
                Some(call_id.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(ids.contains(&"cid1"), "cid1 must be in function_call_outputs");
    assert!(ids.contains(&"cid2"), "cid2 must be in function_call_outputs");
}

/// When an execution backend is configured and a wasm-runtime is registered,
/// `bash` must appear in the tools array sent to xAI.
#[tokio::test]
async fn execution_backend_registered_adds_bash_to_tools_array() {
    use async_nats::jetstream;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(text_response_with_id("ok", "resp_1"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert!(
        calls[0].tools.iter().any(|t| t.name() == "bash"),
        "bash must appear in tools when execution backend is registered"
    );
}

// ── bash feature: final gap coverage ─────────────────────────────────────────

/// `continuation_in_progress = true` is set after executing bash. If the
/// follow-up HTTP call fails with a 5xx error the agent must NOT retry with
/// full history — doing so would discard the `function_call_output` items.
/// Assert: `prompt` returns an error and only 2 HTTP calls were made (no retry).
#[tokio::test]
async fn continuation_in_progress_prevents_stale_id_retry_after_bash() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();

    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new("tid-1"))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new("ok\n".to_string(), false)).unwrap()
            } else { vec![] };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // Round 1: bash call — sets continuation_in_progress = true.
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo ok"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    // Round 2: 5xx error — retry guard must be blocked by continuation_in_progress.
    mock.push_response(error_response());

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let result = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run bash"))],
        ))
        .await;

    assert!(result.is_err(), "5xx after bash round must surface as an error");
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "no stale-ID retry must occur when continuation_in_progress; got {} calls",
        calls.len()
    );
}

/// After bash rounds complete and the final model response carries a ResponseId,
/// `last_response_id` must be updated to that ID so subsequent prompts can use
/// stateful multi-turn via `previous_response_id`.
#[tokio::test]
async fn last_response_id_updated_when_bash_ran_and_final_round_has_response_id() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();

    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new("tid-1"))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new("ok\n".to_string(), false)).unwrap()
            } else { vec![] };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    // Round 1: bash call.
    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo ok"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    // Round 2: final response WITH a ResponseId — must be persisted.
    mock.push_response(text_response_with_id("done", "resp_final"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();
    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("run it"))],
        ))
        .await
        .unwrap();

    assert_eq!(
        agent.test_last_response_id(&session_id).await,
        Some("resp_final".to_string()),
        "last_response_id must be updated to the final round's ResponseId after bash"
    );
}

/// When `execution_nats` is set but no wasm-runtime capability is registered
/// in the registry, `reg.discover(\"execution\")` returns empty and `bash` must
/// not appear in the tools array.
#[tokio::test]
async fn execution_backend_set_but_no_wasm_runtime_registered_means_no_bash_in_tools() {
    use async_nats::jetstream;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    // Registry provisioned but no capability registered.
    let registry = trogon_registry::Registry::new(reg_store);

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(text_response_with_id("ok", "resp_1"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    assert!(
        !calls[0].tools.iter().any(|t| t.name() == "bash"),
        "bash must not appear in tools when no wasm-runtime is registered in the registry"
    );
}

/// When the wasm-runtime registry entry has no `acp_prefix` field in its
/// metadata, the agent must fall back to `"acp.wasm"` as the NATS subject
/// prefix and successfully route terminal requests there.
#[tokio::test]
async fn wasm_prefix_falls_back_to_acp_wasm_when_metadata_missing() {
    use agent_client_protocol::{
        CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
        WaitForTerminalExitResponse,
    };
    use async_nats::jetstream;
    use futures_util::StreamExt as _;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_xai_runner::InputItem;

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    // Register without acp_prefix — forces the fallback path.
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "acp.wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({}), // no acp_prefix field
    }).await.unwrap();

    // Terminal responder on the fallback prefix "acp.wasm".
    let nats_srv = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_srv.subscribe("acp.wasm.session.>").await.unwrap();
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() { Some(r) => r, None => continue };
            let subject: &str = msg.subject.as_ref();
            let payload: Vec<u8> = if subject.ends_with(".create") {
                serde_json::to_vec(&CreateTerminalResponse::new(TerminalId::new("tid-1"))).unwrap()
            } else if subject.ends_with(".wait_for_exit") {
                serde_json::to_vec(&WaitForTerminalExitResponse::new(
                    TerminalExitStatus::new().exit_code(Some(0)),
                )).unwrap()
            } else if subject.ends_with(".output") {
                serde_json::to_vec(&TerminalOutputResponse::new("fallback\n".to_string(), false)).unwrap()
            } else { vec![] };
            let _ = nats_srv.publish(reply, payload.into()).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(vec![
        XaiEvent::ResponseId { id: "resp_bash".to_string() },
        XaiEvent::FunctionCall {
            call_id: "cid1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo fallback"}"#.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]);
    mock.push_response(text_response_with_id("ok", "resp_2"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let resp = agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("run fallback"))],
        ))
        .await
        .unwrap();

    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    // The bash output from the fallback prefix responder must reach the model.
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let fco = calls[1].input.iter().find(
        |i| matches!(i, InputItem::FunctionCallOutput { call_id, .. } if call_id == "cid1"),
    );
    assert!(fco.is_some(), "function_call_output must be present — fallback prefix must have been used");
    if let Some(InputItem::FunctionCallOutput { output, .. }) = fco {
        assert!(
            output.contains("fallback"),
            "bash output via fallback prefix must be forwarded; got: {output:?}"
        );
    }
}

// ── ext_method ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ext_method_list_children_returns_forked_sessions() {
    use agent_client_protocol::ExtRequest;
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let parent = agent.new_session(NewSessionRequest::new("/parent")).await.unwrap();
    let parent_id = parent.session_id.to_string();

    let fork1 = agent.fork_session(ForkSessionRequest::new(parent_id.clone(), "/f1")).await.unwrap();
    let fork2 = agent.fork_session(ForkSessionRequest::new(parent_id.clone(), "/f2")).await.unwrap();
    let fork1_id = fork1.session_id.to_string();
    let fork2_id = fork2.session_id.to_string();

    let raw = serde_json::value::RawValue::from_string(
        serde_json::json!({"sessionId": parent_id}).to_string(),
    ).unwrap();
    let resp = agent.ext_method(ExtRequest::new("session/list_children", std::sync::Arc::from(raw))).await.unwrap();

    let result: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
    let mut children: Vec<String> = serde_json::from_value(result["children"].clone()).unwrap();
    children.sort();

    let mut expected = vec![fork1_id, fork2_id];
    expected.sort();

    assert_eq!(children, expected, "both forked sessions must be listed as children");
}

#[tokio::test]
async fn ext_method_unknown_method_returns_method_not_found() {
    use agent_client_protocol::{ErrorCode, ExtRequest};
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let err = agent
        .ext_method(ExtRequest::new("no.such/method", std::sync::Arc::from(raw)))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        ErrorCode::MethodNotFound.into(),
        "unknown ext method must return MethodNotFound: {err:?}"
    );
}

// ── set_session_model side-effects ────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_clears_last_response_id() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response_with_id("hello", "resp_abc"));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hi"))],
        ))
        .await
        .unwrap();
    assert_eq!(agent.test_last_response_id(&session_id).await, Some("resp_abc".to_string()));

    agent
        .set_session_model(SetSessionModelRequest::new(session_id.clone(), "grok-3-mini"))
        .await
        .unwrap();

    assert_eq!(
        agent.test_last_response_id(&session_id).await,
        None,
        "set_session_model must clear last_response_id to prevent stale ID on next prompt"
    );
}

// ── fork_session with branchAtIndex ──────────────────────────────────────────

#[tokio::test]
async fn fork_session_with_branch_at_index_truncates_history() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["r1"]));
    mock.push_response(text_response(&["r2"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let src = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let src_id = src.session_id.to_string();

    agent.prompt(PromptRequest::new(src_id.clone(), vec![ContentBlock::Text(TextContent::new("a"))])).await.unwrap();
    agent.prompt(PromptRequest::new(src_id.clone(), vec![ContentBlock::Text(TextContent::new("b"))])).await.unwrap();

    let src_history = agent.test_session_history(&src_id).await;
    assert_eq!(src_history.len(), 4, "source: 4 messages");

    let mut meta = serde_json::Map::new();
    meta.insert("branchAtIndex".to_string(), serde_json::json!(2));
    let fork = agent
        .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork").meta(meta))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    let fork_history = agent.test_session_history(&fork_id).await;
    assert_eq!(fork_history.len(), 2, "fork at index 2 must have only first 2 messages; got: {fork_history:?}");
    assert_eq!(fork_history[0].content_str(), "a");
    assert_eq!(fork_history[1].content_str(), "r1");
}

#[tokio::test]
async fn fork_session_parent_and_branch_info_visible_in_list_sessions() {
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(Arc::new(MockXaiHttpClient::new())).await;

    let parent = agent.new_session(NewSessionRequest::new("/parent")).await.unwrap();
    let parent_id = parent.session_id.to_string();

    let plain_fork = agent
        .fork_session(ForkSessionRequest::new(parent_id.clone(), "/plain"))
        .await
        .unwrap();
    let plain_fork_id = plain_fork.session_id.to_string();

    let mut branch_meta = serde_json::Map::new();
    branch_meta.insert("branchAtIndex".to_string(), serde_json::json!(2));
    let branch_fork = agent
        .fork_session(ForkSessionRequest::new(parent_id.clone(), "/branch").meta(branch_meta))
        .await
        .unwrap();
    let branch_fork_id = branch_fork.session_id.to_string();

    let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();

    let session_meta = |target_id: &str| {
        list.sessions
            .iter()
            .find(|s| s.session_id.to_string() == target_id)
            .and_then(|s| s.meta.clone())
    };

    let plain_meta = session_meta(&plain_fork_id).expect("plain fork must have session meta");
    assert_eq!(
        plain_meta.get("parentSessionId").and_then(|v| v.as_str()),
        Some(parent_id.as_str()),
        "plain fork must report parentSessionId"
    );
    assert!(
        plain_meta.get("branchedAtIndex").is_none(),
        "plain fork must not report branchedAtIndex"
    );

    let branch_meta = session_meta(&branch_fork_id).expect("branch fork must have session meta");
    assert_eq!(
        branch_meta.get("parentSessionId").and_then(|v| v.as_str()),
        Some(parent_id.as_str()),
        "branch fork must report parentSessionId"
    );
    assert_eq!(
        branch_meta.get("branchedAtIndex").and_then(|v| v.as_u64()),
        Some(2),
        "branch fork must report branchedAtIndex"
    );

    assert!(
        session_meta(&parent_id).is_none(),
        "root session must not have fork meta in list_sessions"
    );
}

// ── token usage notification ──────────────────────────────────────────────────

#[tokio::test]
async fn usage_update_notification_sent_when_xai_reports_token_usage() {
    use agent_client_protocol::SessionUpdate;
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_with_usage("Hello"));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hi"))],
        ))
        .await
        .unwrap();

    let notifs = agent.test_notifier().notifications.lock().unwrap();
    let has_usage = notifs
        .iter()
        .any(|n| matches!(&n.update, SessionUpdate::UsageUpdate(_)));
    assert!(has_usage, "a UsageUpdate notification must be sent when xAI reports token usage; got: {notifs:?}");
}

// ── FinishReason::Other ───────────────────────────────────────────────────────

#[tokio::test]
async fn finish_reason_other_treated_as_end_turn_no_error() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(vec![
        XaiEvent::TextDelta { text: "hello".to_string() },
        XaiEvent::Finished {
            reason: FinishReason::Other("some_future_status".to_string()),
            incomplete_reason: None,
        },
        XaiEvent::Done,
    ]);
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    let resp = agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("hi"))],
        ))
        .await
        .expect("unknown finish reason must not cause an error");

    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2, "user + assistant must be in history: {history:?}");
    assert_eq!(history[1].content_str(), "hello");
}

// ── binary blob resource ──────────────────────────────────────────────────────

#[tokio::test]
async fn binary_blob_resource_in_prompt_is_included_as_text() {
    use agent_client_protocol::BlobResourceContents;
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["ok"]));
    let agent = make_agent(Arc::clone(&mock)).await;

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::BlobResourceContents(
                    BlobResourceContents::new("aGVsbG8=", "file:///data.bin")
                        .mime_type("application/octet-stream"),
                ),
            ))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let content = calls[0].input.last().unwrap().content().unwrap_or("");
    assert_eq!(
        content,
        "[Binary resource: file:///data.bin (application/octet-stream)]",
        "binary blob must be forwarded as a text placeholder"
    );
}

// ── trim_history orphaned user ────────────────────────────────────────────────

#[tokio::test]
async fn trim_history_drops_orphaned_user_message_individually() {
    let _guard = env_lock().lock().unwrap();
    let mock = Arc::new(MockXaiHttpClient::new());
    mock.push_response(text_response(&["reply"]));
    let agent = make_agent(Arc::clone(&mock)).await.with_max_history(2);

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.to_string();

    // Plant [orphan-user, second-user, second-assistant] — starts with two user
    // messages to trigger the orphaned-user trim path in trim_history.
    agent
        .test_set_session_history(
            &session_id,
            vec![
                trogon_xai_runner::Message::user("orphan"),
                trogon_xai_runner::Message::user("second"),
                trogon_xai_runner::Message::assistant_text("second-reply"),
            ],
        )
        .await;

    agent
        .prompt(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new("new"))],
        ))
        .await
        .unwrap();

    // History before trim: [orphan, second, second-reply, new, reply] (5 items)
    // trim_history(5, max=2):
    //   (user "orphan", user "second") → orphaned user, drain 1 → 4 items
    //   (user "second", assistant "second-reply") → normal pair, drain 2 → 2 items
    let history = agent.test_session_history(&session_id).await;
    assert_eq!(
        history.len(),
        2,
        "orphaned user must be dropped individually; expected [new, reply]; got: {history:?}"
    );
    assert_eq!(history[0].content_str(), "new");
    assert_eq!(history[1].content_str(), "reply");
}

/// The `bash` tool spec sent to xAI must have a `parameters` schema with a
/// required `command` string property — this is the contract the model uses
/// to call the tool.
#[tokio::test]
async fn bash_tool_spec_has_correct_parameters_schema() {
    use async_nats::jetstream;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_xai_runner::ToolSpec;

    let _container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start NATS — is Docker running?");
    let port = _container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);
    registry.register(&trogon_registry::AgentCapability {
        agent_type: "wasm-runtime".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "wasm" }),
    }).await.unwrap();

    let mock = Arc::new(MockXaiHttpClient::new());
    let _guard = env_lock().lock().unwrap();
    let agent = make_agent(mock.clone())
        .await
        .with_execution_backend(nats.clone(), registry);

    mock.push_response(text_response_with_id("ok", "resp_1"));

    let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    agent
        .prompt(PromptRequest::new(
            sess.session_id.to_string(),
            vec![ContentBlock::Text(TextContent::new("hello"))],
        ))
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap();
    let bash = calls[0]
        .tools
        .iter()
        .find(|t| t.name() == "bash")
        .expect("bash must be in tools");

    if let ToolSpec::Function { parameters, .. } = bash {
        assert_eq!(
            parameters["type"], "object",
            "bash parameters root must be type:object"
        );
        assert_eq!(
            parameters["properties"]["command"]["type"], "string",
            "command property must be type:string"
        );
        assert_eq!(
            parameters["required"][0], "command",
            "command must be listed as required"
        );
    } else {
        panic!("bash tool must be ToolSpec::Function, not ServerSide");
    }
}
