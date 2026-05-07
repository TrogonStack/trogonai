//! End-to-end tests for `OpenRouterClient` using a real local axum HTTP server.
//!
//! These tests exercise the actual HTTP stack, SSE parser, and event mapping
//! without requiring a real OpenRouter API key or Docker.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test live_http

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::Bytes;
use futures_util::StreamExt as _;
use tokio::net::TcpListener;

use trogon_openrouter_runner::{FinishReason, Message, OpenRouterClient, OpenRouterEvent};
use trogon_openrouter_runner::OpenRouterHttpClient as _;

// ── Shared server state ───────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct ServerState {
    responses: Arc<Mutex<VecDeque<(StatusCode, Bytes)>>>,
    captured: Arc<Mutex<Vec<Captured>>>,
}

struct Captured {
    auth: String,
    body: serde_json::Value,
}

impl ServerState {
    fn push_sse(&self, body: impl Into<String>) {
        self.responses
            .lock()
            .unwrap()
            .push_back((StatusCode::OK, Bytes::from(body.into())));
    }

    fn push_error(&self, status: StatusCode, body: impl Into<String>) {
        self.responses
            .lock()
            .unwrap()
            .push_back((status, Bytes::from(body.into())));
    }

    fn last_auth(&self) -> Option<String> {
        self.captured.lock().unwrap().last().map(|c| c.auth.clone())
    }

    fn last_body(&self) -> Option<serde_json::Value> {
        self.captured.lock().unwrap().last().map(|c| c.body.clone())
    }
}

async fn chat_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    state.captured.lock().unwrap().push(Captured { auth, body: body_json });

    let (status, resp_body) = state
        .responses
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, Bytes::from("queue empty")));

    if status == StatusCode::OK {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            resp_body,
        )
            .into_response()
    } else {
        (status, resp_body).into_response()
    }
}

// ── TestServer ────────────────────────────────────────────────────────────────

struct TestServer {
    base_url: String,
    state: ServerState,
    _handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn new() -> Self {
        let state = ServerState::default();
        let app = Router::new()
            .route("/chat/completions", post(chat_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            state,
            _handle: handle,
        }
    }

    fn client(&self) -> OpenRouterClient {
        OpenRouterClient::with_base_url(&self.base_url)
    }

    async fn run(&self, model: &str, api_key: &str) -> Vec<OpenRouterEvent> {
        self.client()
            .chat_stream(model, &[Message::user("hello")], api_key)
            .await
            .collect()
            .await
    }
}

// ── SSE body builder ──────────────────────────────────────────────────────────

fn sse(lines: &[&str]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn happy_path_text_delta_finish_usage_done() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"Hello, world!"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "Hello, world!"));
    assert!(matches!(&events[1], OpenRouterEvent::Finished { reason: FinishReason::Stop }));
    assert!(matches!(&events[2], OpenRouterEvent::Usage { prompt_tokens: 10, completion_tokens: 3 }));
    assert!(matches!(&events[3], OpenRouterEvent::Done));
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_text_deltas_are_all_received() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{"content":", "},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{"content":"world"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    let texts: Vec<&str> = events
        .iter()
        .filter_map(|e| {
            if let OpenRouterEvent::TextDelta { text } = e {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(texts, ["Hello", ", ", "world"]);
    assert!(events.iter().any(|e| matches!(e, OpenRouterEvent::Done)));
}

#[tokio::test(flavor = "current_thread")]
async fn error_event_on_unauthorized_response() {
    let srv = TestServer::new().await;
    srv.state.push_error(
        StatusCode::UNAUTHORIZED,
        r#"{"error":{"message":"Invalid API key","type":"auth_error"}}"#,
    );

    let events = srv.run("test-model", "bad-key").await;

    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("401")),
        "expected Error containing 401, got: {:?}",
        events
    );
}

#[tokio::test(flavor = "current_thread")]
async fn error_event_on_server_error_response() {
    let srv = TestServer::new().await;
    srv.state.push_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{"error":"internal server error"}"#,
    );

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("500")),
        "expected Error containing 500, got: {:?}",
        events
    );
}

#[tokio::test(flavor = "current_thread")]
async fn error_event_on_rate_limit_response() {
    let srv = TestServer::new().await;
    srv.state.push_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"Rate limit exceeded"}}"#,
    );

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("429")),
        "expected Error containing 429, got: {:?}",
        events
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bearer_token_is_sent_in_authorization_header() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&["data: [DONE]"]));

    srv.run("test-model", "my-secret-key").await;

    let auth = srv.state.last_auth().expect("no request captured");
    assert_eq!(auth, "Bearer my-secret-key");
}

#[tokio::test(flavor = "current_thread")]
async fn request_body_contains_model_and_messages() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&["data: [DONE]"]));

    let client = srv.client();
    let messages = vec![
        Message::user("What is 2+2?"),
        Message::assistant("4"),
        Message::user("And 3+3?"),
    ];
    client
        .chat_stream("anthropic/claude-sonnet-4-6", &messages, "sk-test")
        .await
        .collect::<Vec<_>>()
        .await;

    let body = srv.state.last_body().expect("no request captured");
    assert_eq!(body["model"], "anthropic/claude-sonnet-4-6");
    assert_eq!(body["stream"], true);

    let msgs = body["messages"].as_array().expect("messages must be array");
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "What is 2+2?");
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"], "4");
    assert_eq!(msgs[2]["role"], "user");
    assert_eq!(msgs[2]["content"], "And 3+3?");
}

#[tokio::test(flavor = "current_thread")]
async fn request_body_does_not_include_token_counts() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&["data: [DONE]"]));

    let client = srv.client();
    let messages = vec![Message::assistant_with_usage("reply", 100, 50)];
    client
        .chat_stream("test-model", &messages, "sk-test")
        .await
        .collect::<Vec<_>>()
        .await;

    let body = srv.state.last_body().expect("no request captured");
    let msgs = body["messages"].as_array().expect("messages must be array");
    assert!(
        msgs[0].get("prompt_tokens").is_none(),
        "prompt_tokens must be stripped before sending to API"
    );
    assert!(
        msgs[0].get("completion_tokens").is_none(),
        "completion_tokens must be stripped before sending to API"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn empty_content_deltas_are_skipped() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":""},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{"content":"real text"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    let text_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, OpenRouterEvent::TextDelta { .. }))
        .collect();

    assert_eq!(text_events.len(), 1, "empty delta must not produce a TextDelta event");
    assert!(matches!(&text_events[0], OpenRouterEvent::TextDelta { text } if text == "real text"));
}

#[tokio::test(flavor = "current_thread")]
async fn done_sentinel_only_stream() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&["data: [DONE]"]));

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], OpenRouterEvent::Done));
}

#[tokio::test(flavor = "current_thread")]
async fn finish_reason_length_is_parsed() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"truncated..."},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, OpenRouterEvent::Finished { reason: FinishReason::Length })),
        "expected Finished with reason=Length"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn finish_reason_unknown_becomes_other() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    assert!(
        events.iter().any(|e| matches!(
            e,
            OpenRouterEvent::Finished { reason: FinishReason::Other(s) } if s == "content_filter"
        )),
        "expected Finished with reason=Other(\"content_filter\")"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn usage_in_separate_chunk_after_finish() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"Hi"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        r#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":2,"total_tokens":9}}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "Hi"));
    assert!(matches!(&events[1], OpenRouterEvent::Finished { reason: FinishReason::Stop }));
    assert!(matches!(&events[2], OpenRouterEvent::Usage { prompt_tokens: 7, completion_tokens: 2 }));
    assert!(matches!(&events[3], OpenRouterEvent::Done));
}

#[tokio::test(flavor = "current_thread")]
async fn keep_alive_comment_lines_are_ignored() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        ": keep-alive",
        r#"data: {"choices":[{"delta":{"content":"Hi"},"finish_reason":null}]}"#,
        ": keep-alive",
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    let non_meta: Vec<_> = events
        .iter()
        .filter(|e| !matches!(e, OpenRouterEvent::Done))
        .collect();
    assert_eq!(non_meta.len(), 1);
    assert!(matches!(non_meta[0], OpenRouterEvent::TextDelta { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_requests_get_independent_responses() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"first"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"second"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let events1 = srv.run("test-model", "sk-test").await;
    let events2 = srv.run("test-model", "sk-test").await;

    assert!(matches!(&events1[0], OpenRouterEvent::TextDelta { text } if text == "first"));
    assert!(matches!(&events2[0], OpenRouterEvent::TextDelta { text } if text == "second"));
}

#[tokio::test(flavor = "current_thread")]
async fn stream_with_crlf_line_endings() {
    let srv = TestServer::new().await;
    // Use \r\n endings explicitly.
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\r\n",
        "data: [DONE]\r\n",
    );
    srv.state.push_sse(body);

    let events = srv.run("test-model", "sk-test").await;

    assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "Hi"));
    assert!(matches!(&events[1], OpenRouterEvent::Done));
}

#[tokio::test(flavor = "current_thread")]
async fn request_includes_stream_options_with_usage() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&["data: [DONE]"]));

    srv.run("test-model", "sk-test").await;

    let body = srv.state.last_body().expect("no request captured");
    assert_eq!(
        body["stream_options"]["include_usage"],
        true,
        "stream_options.include_usage must be true"
    );
}
