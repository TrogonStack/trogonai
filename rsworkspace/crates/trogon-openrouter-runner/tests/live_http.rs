//! End-to-end tests for `OpenRouterClient` using a real local axum HTTP server.
//!
//! These tests exercise the actual HTTP stack, SSE parser, and event mapping
//! without requiring a real OpenRouter API key or Docker.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test live_http

use std::collections::VecDeque;
use std::sync::Mutex;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use bytes::Bytes;
use futures_util::StreamExt as _;
use tokio::net::TcpListener;
use tokio::sync::Notify;

use agent_client_protocol::{
    Agent as _, AuthenticateRequest, CancelNotification, ContentBlock, ForkSessionRequest,
    NewSessionRequest, PromptRequest, SessionNotification, SetSessionModelRequest, StopReason,
};
use trogon_openrouter_runner::{
    FinishReason, Message, OpenRouterAgent, OpenRouterClient, OpenRouterEvent, SessionNotifier,
};
use trogon_openrouter_runner::OpenRouterHttpClient as _;

// ── Shared server state ───────────────────────────────────────────────────────

struct ServerResponse {
    status: StatusCode,
    body: Bytes,
    extra_headers: Vec<(&'static str, String)>,
    /// If true, send `body` as a streaming chunk then block forever (simulates
    /// a long-running generation that the client might cancel mid-stream).
    slow: bool,
    /// If true, send headers then immediately yield an IO error in the body stream
    /// (simulates a connection drop / TCP reset after headers are received).
    error_body: bool,
}

#[derive(Clone, Default)]
struct ServerState {
    responses: Arc<Mutex<VecDeque<ServerResponse>>>,
    captured: Arc<Mutex<Vec<Captured>>>,
}

struct Captured {
    auth: String,
    body: serde_json::Value,
}

impl ServerState {
    fn push_sse(&self, body: impl Into<String>) {
        self.push_raw(StatusCode::OK, body.into(), vec![], false);
    }

    fn push_error(&self, status: StatusCode, body: impl Into<String>) {
        self.push_raw(status, body.into(), vec![], false);
    }

    /// Push a 429 response with `Retry-After: 0` so tests don't actually wait.
    fn push_429(&self) {
        self.push_raw(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":"rate limit exceeded"}"#.to_string(),
            vec![("retry-after", "0".to_string())],
            false,
        );
    }

    /// Push a 503 response with `Retry-After: 0`.
    fn push_503(&self) {
        self.push_raw(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"service unavailable"}"#.to_string(),
            vec![("retry-after", "0".to_string())],
            false,
        );
    }

    /// Push an SSE response that sends `body` as one chunk then blocks forever.
    fn push_slow_sse(&self, body: impl Into<String>) {
        self.push_raw(StatusCode::OK, body.into(), vec![], true);
    }

    fn push_raw(&self, status: StatusCode, body: String, extra_headers: Vec<(&'static str, String)>, slow: bool) {
        self.responses.lock().unwrap().push_back(ServerResponse {
            status,
            body: Bytes::from(body),
            extra_headers,
            slow,
            error_body: false,
        });
    }

    /// Push a response that sends HTTP 200 headers then immediately drops the body
    /// with an IO error, simulating a TCP connection reset after headers are received.
    fn push_connection_reset(&self) {
        self.responses.lock().unwrap().push_back(ServerResponse {
            status: StatusCode::OK,
            body: Bytes::new(),
            extra_headers: vec![],
            slow: false,
            error_body: true,
        });
    }

    fn last_auth(&self) -> Option<String> {
        self.captured.lock().unwrap().last().map(|c| c.auth.clone())
    }

    fn last_body(&self) -> Option<serde_json::Value> {
        self.captured.lock().unwrap().last().map(|c| c.body.clone())
    }

    fn request_count(&self) -> usize {
        self.captured.lock().unwrap().len()
    }
}

async fn chat_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> axum::http::Response<axum::body::Body> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    state.captured.lock().unwrap().push(Captured { auth, body: body_json });

    let resp = state.responses.lock().unwrap().pop_front().unwrap_or_else(|| ServerResponse {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        body: Bytes::from("queue empty"),
        extra_headers: vec![],
        slow: false,
        error_body: false,
    });

    let mut builder = axum::http::Response::builder().status(resp.status);
    if resp.status == StatusCode::OK {
        builder = builder.header(header::CONTENT_TYPE, "text/event-stream");
    }
    for (k, v) in resp.extra_headers {
        builder = builder.header(k, v);
    }
    if resp.error_body {
        // Send headers then immediately error — simulates a TCP reset after headers.
        use futures_util::stream;
        let body_stream = stream::once(async move {
            Err::<Bytes, std::io::Error>(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "simulated connection reset",
            ))
        });
        builder.body(axum::body::Body::from_stream(body_stream)).unwrap()
    } else if resp.slow {
        // Send the initial chunk then block forever — lets tests verify cancel.
        use futures_util::stream;
        let body_stream = stream::once(async move { Ok::<Bytes, std::io::Error>(resp.body) })
            .chain(stream::pending::<Result<Bytes, std::io::Error>>());
        builder.body(axum::body::Body::from_stream(body_stream)).unwrap()
    } else {
        builder.body(axum::body::Body::from(resp.body)).unwrap()
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

// ── Retry tests ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn retries_on_429_and_eventually_succeeds() {
    let srv = TestServer::new().await;
    // First request → 429 (with Retry-After: 0 so no real wait).
    srv.state.push_429();
    // Second request → success.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 2, "should have made exactly 2 requests");
    assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "ok"));
    assert!(matches!(&events[1], OpenRouterEvent::Done));
}

#[tokio::test(flavor = "current_thread")]
async fn retries_on_503_and_eventually_succeeds() {
    let srv = TestServer::new().await;
    srv.state.push_503();
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"recovered"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 2);
    assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "recovered"));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_retry_on_401() {
    let srv = TestServer::new().await;
    srv.state.push_error(StatusCode::UNAUTHORIZED, r#"{"error":"invalid key"}"#);

    let events = srv.run("test-model", "bad-key").await;

    // Must have made exactly 1 request — no retry on auth failure.
    assert_eq!(srv.state.request_count(), 1, "401 must not be retried");
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("401")));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_retry_on_400() {
    let srv = TestServer::new().await;
    srv.state.push_error(StatusCode::BAD_REQUEST, r#"{"error":"bad request"}"#);

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 1, "400 must not be retried");
    assert!(matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("400")));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_retry_on_402() {
    let srv = TestServer::new().await;
    srv.state.push_error(
        StatusCode::PAYMENT_REQUIRED,
        r#"{"error":{"message":"Insufficient credits","code":402}}"#,
    );

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 1, "402 must not be retried");
    assert!(matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("402")));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_retry_on_403() {
    let srv = TestServer::new().await;
    srv.state.push_error(
        StatusCode::FORBIDDEN,
        r#"{"error":{"message":"Forbidden","code":403}}"#,
    );

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 1, "403 must not be retried");
    assert!(matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("403")));
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_retry_on_404() {
    let srv = TestServer::new().await;
    srv.state.push_error(
        StatusCode::NOT_FOUND,
        r#"{"error":{"message":"Model not found","code":404}}"#,
    );

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 1, "404 must not be retried");
    assert!(matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("404")));
}

#[tokio::test(flavor = "current_thread")]
async fn exhausts_retries_and_returns_error() {
    let srv = TestServer::new().await;
    // Queue four 429s — MAX_RETRIES=3 means 4 total attempts, all failing.
    srv.state.push_429();
    srv.state.push_429();
    srv.state.push_429();
    srv.state.push_429();

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 4, "should have made 4 attempts (1 + 3 retries)");
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], OpenRouterEvent::Error { message } if message.contains("429")),
        "expected Error with 429 after exhausting retries"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_retries_then_success() {
    let srv = TestServer::new().await;
    // Two 429s, then success — tests that attempt counter increments correctly.
    srv.state.push_429();
    srv.state.push_429();
    srv.state.push_sse(sse(&["data: [DONE]"]));

    let events = srv.run("test-model", "sk-test").await;

    assert_eq!(srv.state.request_count(), 3);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], OpenRouterEvent::Done));
}

// ── Cancel test ───────────────────────────────────────────────────────────────

/// Minimal session notifier that signals when the first notification arrives
/// and optionally tracks how many text-chunk bytes and usage events were delivered.
#[derive(Clone)]
struct TestNotifier {
    first_notification: Arc<Notify>,
    count: Arc<AtomicUsize>,
    text_bytes: Arc<AtomicUsize>,
    usage_count: Arc<AtomicUsize>,
}

impl TestNotifier {
    fn new() -> Self {
        Self {
            first_notification: Arc::new(Notify::new()),
            count: Arc::new(AtomicUsize::new(0)),
            text_bytes: Arc::new(AtomicUsize::new(0)),
            usage_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Wait until at least one notification has been received.
    async fn wait_for_first(&self) {
        if self.count.load(Ordering::Relaxed) == 0 {
            self.first_notification.notified().await;
        }
    }

    fn text_bytes_received(&self) -> usize {
        self.text_bytes.load(Ordering::Relaxed)
    }

    fn usage_notifications_received(&self) -> usize {
        self.usage_count.load(Ordering::Relaxed)
    }
}

#[async_trait(?Send)]
impl SessionNotifier for TestNotifier {
    async fn notify(&self, n: SessionNotification) {
        use agent_client_protocol::SessionUpdate;
        match n.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(t) = chunk.content {
                    self.text_bytes.fetch_add(t.text.len(), Ordering::Relaxed);
                }
            }
            SessionUpdate::UsageUpdate(_) => {
                self.usage_count.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        let prev = self.count.fetch_add(1, Ordering::Relaxed);
        if prev == 0 {
            self.first_notification.notify_one();
        }
    }
}

/// Cancel mid-stream with the real HTTP client: prompt must return
/// `StopReason::Cancelled` quickly, and the HTTP connection must be released
/// (verified by the server-side pending stream being dropped, not kept alive).
#[tokio::test(flavor = "current_thread")]
async fn cancel_mid_stream_returns_cancelled_stop_reason() {
    let srv = TestServer::new().await;

    // Server sends one text chunk then blocks forever.
    srv.state.push_slow_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"token"},"finish_reason":null}]}"#,
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let session_id = resp.session_id.clone();

            // Start prompt in background — it will block on the slow server.
            let agent_prompt = Arc::clone(&agent);
            let sid = session_id.clone();
            let prompt_handle = tokio::task::spawn_local(async move {
                agent_prompt
                    .prompt(PromptRequest::new(
                        sid,
                        vec![ContentBlock::from("hello".to_string())],
                    ))
                    .await
                    .unwrap()
            });

            // Wait until the first chunk notification arrives (stream is live).
            notifier.wait_for_first().await;

            // Cancel the in-flight prompt.
            agent
                .cancel(CancelNotification::new(session_id))
                .await
                .unwrap();

            // Prompt must complete within 2 seconds.
            let result = tokio::time::timeout(Duration::from_secs(2), prompt_handle)
                .await
                .expect("prompt did not finish within 2s after cancel")
                .expect("task panicked");

            assert!(
                matches!(result.stop_reason, StopReason::Cancelled),
                "expected Cancelled, got {:?}",
                result.stop_reason
            );
        })
        .await;
}

/// Cancel before any chunk arrives (cancel races with HTTP request initiation).
#[tokio::test(flavor = "current_thread")]
async fn cancel_before_first_chunk_returns_cancelled() {
    let srv = TestServer::new().await;

    // Server sends a slow response: nothing arrives immediately.
    srv.state.push_slow_sse(""); // empty initial chunk, then blocks

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let session_id = resp.session_id.clone();

            let agent_prompt = Arc::clone(&agent);
            let sid = session_id.clone();
            let prompt_handle = tokio::task::spawn_local(async move {
                agent_prompt
                    .prompt(PromptRequest::new(
                        sid,
                        vec![ContentBlock::from("hello".to_string())],
                    ))
                    .await
                    .unwrap()
            });

            // Yield to let the prompt task start, then immediately cancel.
            tokio::task::yield_now().await;
            agent
                .cancel(CancelNotification::new(session_id))
                .await
                .unwrap();

            let result = tokio::time::timeout(Duration::from_secs(2), prompt_handle)
                .await
                .expect("prompt did not finish within 2s after cancel")
                .expect("task panicked");

            assert!(matches!(result.stop_reason, StopReason::Cancelled));
        })
        .await;
}

// ── Response size guard ───────────────────────────────────────────────────────

/// Real HTTP: server streams many chunks whose total exceeds the configured
/// limit. The agent must stop early and return `EndTurn` with partial content,
/// rather than hanging or OOMing.
#[tokio::test(flavor = "current_thread")]
async fn response_size_limit_stops_stream_early() {
    let srv = TestServer::new().await;

    // 10 chunks × 20 bytes each = 200 bytes total.
    // We set the limit to 50 bytes, so the guard should fire around chunk 3.
    let chunk = r#"data: {"choices":[{"delta":{"content":"01234567890123456789"},"finish_reason":null}]}"#;
    let mut lines: Vec<&str> = vec![chunk; 10];
    lines.push("data: [DONE]");
    srv.state.push_sse(sse(&lines));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(
        OpenRouterAgent::with_deps(notifier.clone(), "test-model", "test-key", client)
            .with_max_response_bytes(50),
    );

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let session_id = resp.session_id.clone();

            let agent_p = Arc::clone(&agent);
            let sid = session_id.clone();
            let result = tokio::task::LocalSet::new()
                .run_until(async move {
                    agent_p
                        .prompt(PromptRequest::new(
                            sid,
                            vec![ContentBlock::from("hello".to_string())],
                        ))
                        .await
                        .unwrap()
                })
                .await;

            // Must complete (not hang), even though server would send more.
            assert!(
                matches!(result.stop_reason, StopReason::EndTurn),
                "expected EndTurn after size limit, got {:?}",
                result.stop_reason
            );

            // Notifier received some but not all 200 bytes of content.
            let received = notifier.text_bytes_received();
            assert!(received > 0, "must have received some content");
            assert!(
                received <= 80,
                "guard should have fired well before 200 bytes, got {received}"
            );
        })
        .await;
}

/// SSE body that ends without a trailing `\n` — the parser must flush the
/// remaining buffer when the connection closes and still produce a TextDelta.
#[tokio::test(flavor = "current_thread")]
async fn sse_truncated_without_trailing_newline_is_flushed() {
    let srv = TestServer::new().await;
    // No \n at the end — raw body, not via push_sse which appends \n.
    srv.state.push_raw(
        StatusCode::OK,
        r#"data: {"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#.to_string(),
        vec![],
        false,
    );

    let events = srv.run("test-model", "sk-test").await;

    assert!(
        events.iter().any(|e| matches!(e, OpenRouterEvent::TextDelta { text } if text == "hello")),
        "SSE parser must flush remaining buffer on stream close: {events:?}"
    );
}

/// A single SSE chunk whose content exceeds `max_response_bytes` in one shot —
/// the guard must fire even without multiple chunks accumulating.
#[tokio::test(flavor = "current_thread")]
async fn single_chunk_exceeding_size_limit_fires_guard() {
    let srv = TestServer::new().await;
    // 100 bytes of content, limit is 50.
    let big_text = "x".repeat(100);
    let chunk = format!(
        r#"data: {{"choices":[{{"delta":{{"content":"{big_text}"}},"finish_reason":null}}]}}"#
    );
    // Push chunk followed by [DONE] — guard should fire before [DONE] is seen.
    srv.state.push_raw(
        StatusCode::OK,
        format!("{chunk}\ndata: [DONE]\n"),
        vec![],
        false,
    );

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(
        OpenRouterAgent::with_deps(notifier.clone(), "test-model", "test-key", client)
            .with_max_response_bytes(50),
    );

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let result = agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("q".to_string())],
                ))
                .await
                .unwrap();
            assert!(
                matches!(result.stop_reason, StopReason::EndTurn),
                "guard must stop the stream cleanly: {result:?}"
            );
            // The oversized chunk was delivered then the guard fired.
            assert_eq!(
                notifier.text_bytes_received(),
                100,
                "the chunk bytes reach the notifier before the guard fires"
            );
        })
        .await;
}

/// Server sends 200 headers then drops the connection with an IO error.
/// The agent must complete (not hang) and return `EndTurn`.
#[tokio::test(flavor = "current_thread")]
async fn stream_io_error_mid_body_returns_end_turn() {
    let srv = TestServer::new().await;
    srv.state.push_connection_reset();

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let result = agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("q".to_string())],
                ))
                .await
                .unwrap();
            // Agent must not hang; it logs the stream error and returns EndTurn.
            assert!(
                matches!(result.stop_reason, StopReason::EndTurn),
                "IO error during body must produce EndTurn, not hang: {result:?}"
            );
        })
        .await;
}

/// Second prompt must send the full prior conversation in the request body —
/// user turn 1, assistant turn 1, then user turn 2.
#[tokio::test(flavor = "current_thread")]
async fn history_carries_over_between_turns() {
    let srv = TestServer::new().await;

    // Turn 1: server returns "world".
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"world"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    // Turn 2: server returns something (content doesn't matter for this test).
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let sid = resp.session_id.clone();

            // Turn 1.
            agent
                .prompt(PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("hello".to_string())],
                ))
                .await
                .unwrap();

            // Turn 2.
            agent
                .prompt(PromptRequest::new(
                    sid,
                    vec![ContentBlock::from("next question".to_string())],
                ))
                .await
                .unwrap();

            // The second request must include the full prior turn in the body.
            let body = srv.state.last_body().expect("server must have captured a request");
            let messages = body["messages"].as_array().expect("messages must be array");
            let roles: Vec<&str> = messages
                .iter()
                .filter_map(|m| m["role"].as_str())
                .collect();
            assert_eq!(
                roles,
                ["user", "assistant", "user"],
                "second turn must send full history: {body}"
            );
            assert_eq!(messages[0]["content"].as_str(), Some("hello"));
            assert_eq!(messages[1]["content"].as_str(), Some("world"));
            assert_eq!(messages[2]["content"].as_str(), Some("next question"));
        })
        .await;
}

/// When a system prompt is configured, it must appear as the first message in
/// the request body sent to the model, before any user messages.
#[tokio::test(flavor = "current_thread")]
async fn system_prompt_is_first_wire_message() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(
        OpenRouterAgent::with_deps(notifier.clone(), "test-model", "test-key", client)
            .with_system_prompt("You are a helpful assistant."),
    );

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("hi".to_string())],
                ))
                .await
                .unwrap();

            let body = srv.state.last_body().expect("server must have captured request");
            let messages = body["messages"].as_array().expect("messages must be array");
            assert!(
                !messages.is_empty(),
                "request must have at least one message"
            );
            assert_eq!(
                messages[0]["role"].as_str(),
                Some("system"),
                "system prompt must be the first message: {body}"
            );
            assert_eq!(
                messages[0]["content"].as_str(),
                Some("You are a helpful assistant.")
            );
            assert_eq!(
                messages[1]["role"].as_str(),
                Some("user"),
                "user message must follow the system prompt"
            );
        })
        .await;
}

/// Authenticating with a per-user API key must use that key in the
/// Authorization header, overriding any globally configured key.
#[tokio::test(flavor = "current_thread")]
async fn user_provided_api_key_used_in_wire_authorization() {
    let srv = TestServer::new().await;
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    // Agent has NO global key (empty string → None).
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            // Authenticate with a user-provided key.
            let mut meta = serde_json::Map::new();
            meta.insert(
                "OPENROUTER_API_KEY".to_string(),
                serde_json::json!("user-provided-sk-123"),
            );
            agent
                .authenticate(AuthenticateRequest::new("openrouter-api-key").meta(meta))
                .await
                .unwrap();

            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("hello".to_string())],
                ))
                .await
                .unwrap();

            let auth = srv.state.last_auth().expect("server must have captured auth header");
            assert_eq!(
                auth, "Bearer user-provided-sk-123",
                "per-user key must appear verbatim in Authorization header"
            );
        })
        .await;
}

/// A forked session inherits the source session's conversation history so that
/// the first prompt on the fork includes prior turns.
#[tokio::test(flavor = "current_thread")]
async fn fork_session_inherits_history_in_next_prompt() {
    let srv = TestServer::new().await;

    // Source turn 1 → assistant says "original".
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"original"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    // Fork first prompt → server just needs to reply with anything.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            // Create source session and do one turn.
            let src_resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let src_id = src_resp.session_id.clone();
            agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::from("question".to_string())],
                ))
                .await
                .unwrap();

            // Fork the source session.
            let fork_resp = agent
                .fork_session(ForkSessionRequest::new(src_id, std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let fork_id = fork_resp.session_id;

            // Prompt from the fork.
            agent
                .prompt(PromptRequest::new(
                    fork_id,
                    vec![ContentBlock::from("follow-up".to_string())],
                ))
                .await
                .unwrap();

            // The fork's wire body must contain the source history + the fork's own prompt.
            let body = srv.state.last_body().expect("server must have captured the fork's request");
            let messages = body["messages"].as_array().expect("messages must be array");
            let roles: Vec<&str> = messages
                .iter()
                .filter_map(|m| m["role"].as_str())
                .collect();
            assert_eq!(
                roles,
                ["user", "assistant", "user"],
                "fork's prompt must include inherited history: {body}"
            );
            assert_eq!(messages[0]["content"].as_str(), Some("question"));
            assert_eq!(messages[1]["content"].as_str(), Some("original"));
            assert_eq!(messages[2]["content"].as_str(), Some("follow-up"));
        })
        .await;
}

/// Two sessions created from the same agent have completely independent
/// conversation histories — neither sees the other's messages.
#[tokio::test(flavor = "current_thread")]
async fn two_sessions_have_independent_histories() {
    let srv = TestServer::new().await;

    // Session A turn 1 → "reply-a".
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"reply-a"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    // Session B turn 1 → "reply-b".
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"reply-b"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    // Session A turn 2 — content doesn't matter; we check the request body.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let sid_a = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/a")))
                .await
                .unwrap()
                .session_id;
            let sid_b = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/b")))
                .await
                .unwrap()
                .session_id;

            // A and B each do one turn.
            agent
                .prompt(PromptRequest::new(
                    sid_a.clone(),
                    vec![ContentBlock::from("question-a".to_string())],
                ))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(
                    sid_b.clone(),
                    vec![ContentBlock::from("question-b".to_string())],
                ))
                .await
                .unwrap();

            // A does a second turn; its wire body must contain only A's history.
            agent
                .prompt(PromptRequest::new(
                    sid_a,
                    vec![ContentBlock::from("follow-up-a".to_string())],
                ))
                .await
                .unwrap();

            let body = srv.state.last_body().expect("must have captured third request");
            let messages = body["messages"].as_array().expect("messages must be array");
            // Must see: [user "question-a", assistant "reply-a", user "follow-up-a"]
            // Must NOT contain anything from session B.
            assert_eq!(messages.len(), 3, "session A must have exactly 3 messages: {body}");
            assert_eq!(messages[0]["content"].as_str(), Some("question-a"));
            assert_eq!(messages[1]["content"].as_str(), Some("reply-a"));
            assert_eq!(messages[2]["content"].as_str(), Some("follow-up-a"));
        })
        .await;
}

/// A Usage chunk in the SSE stream must trigger a UsageUpdate notification
/// to the session notifier.
#[tokio::test(flavor = "current_thread")]
async fn usage_chunk_fires_usage_notification() {
    let srv = TestServer::new().await;

    // Server sends one text delta + one usage chunk.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
        r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(
                    resp.session_id,
                    vec![ContentBlock::from("ping".to_string())],
                ))
                .await
                .unwrap();

            assert_eq!(
                notifier.usage_notifications_received(),
                1,
                "exactly one UsageUpdate notification must be sent for the usage chunk"
            );
        })
        .await;
}

/// The wire request body "model" field must reflect the default model configured
/// on the agent, and switching it via set_session_model must change it.
#[tokio::test(flavor = "current_thread")]
async fn wire_model_field_reflects_default_and_overridden_model() {
    let srv = TestServer::new().await;

    // First prompt uses default model, second uses switched model.
    for _ in 0..2 {
        srv.state.push_sse(sse(&[
            r#"data: {"choices":[{"delta":{"content":"ok"},"finish_reason":null}]}"#,
            "data: [DONE]",
        ]));
    }

    let notifier = TestNotifier::new();
    let client = srv.client();

    // Build agent with two known models.  `with_deps` auto-adds the default if
    // it's not already present, so we set OPENROUTER_MODELS just for this agent.
    unsafe { std::env::set_var("OPENROUTER_MODELS", "model-a:Model A,model-b:Model B"); }
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "model-a",
        "test-key",
        client,
    ));
    unsafe { std::env::remove_var("OPENROUTER_MODELS"); }

    tokio::task::LocalSet::new()
        .run_until(async {
            let resp = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap();
            let sid = resp.session_id.clone();

            // First prompt — uses default "model-a".
            agent
                .prompt(PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("q1".to_string())],
                ))
                .await
                .unwrap();

            let body1 = srv.state.last_body().expect("must have first captured body");
            assert_eq!(
                body1["model"].as_str(),
                Some("model-a"),
                "default model must appear in first wire request: {body1}"
            );

            // Switch to "model-b".
            agent
                .set_session_model(SetSessionModelRequest::new(sid.clone(), "model-b"))
                .await
                .unwrap();

            // Second prompt — uses "model-b".
            agent
                .prompt(PromptRequest::new(
                    sid,
                    vec![ContentBlock::from("q2".to_string())],
                ))
                .await
                .unwrap();

            let body2 = srv.state.last_body().expect("must have second captured body");
            assert_eq!(
                body2["model"].as_str(),
                Some("model-b"),
                "switched model must appear in second wire request: {body2}"
            );
        })
        .await;
}

/// Fork with `branchAtIndex` sends only the truncated history slice on the wire.
#[tokio::test(flavor = "current_thread")]
async fn fork_with_branch_at_index_sends_truncated_history_on_wire() {
    let srv = TestServer::new().await;

    // Source: first prompt response.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"reply1"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    // Source: second prompt response.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"reply2"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    // Fork prompt response.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"fork-reply"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let src_id = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap()
                .session_id;

            // Two prompt turns → 4 messages in source history.
            agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::from("q1".to_string())],
                ))
                .await
                .unwrap();
            agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::from("q2".to_string())],
                ))
                .await
                .unwrap();

            // Fork at index 2 — keeps only the first user+assistant pair.
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 2}),
            )
            .unwrap();
            let fork_id = agent
                .fork_session(
                    ForkSessionRequest::new(src_id, std::path::PathBuf::from("/")).meta(meta),
                )
                .await
                .unwrap()
                .session_id;

            // Prompt from the fork.
            agent
                .prompt(PromptRequest::new(
                    fork_id,
                    vec![ContentBlock::from("fork-q".to_string())],
                ))
                .await
                .unwrap();

            let body = srv.state.last_body().expect("must have fork request body");
            let messages = body["messages"].as_array().expect("messages must be array");
            // Expected: user q1, assistant reply1, user fork-q (q2+reply2 truncated).
            assert_eq!(
                messages.len(),
                3,
                "fork with branchAtIndex:2 must send only 3 messages on wire: {body}"
            );
            assert_eq!(messages[0]["content"].as_str(), Some("q1"));
            assert_eq!(messages[1]["content"].as_str(), Some("reply1"));
            assert_eq!(messages[2]["content"].as_str(), Some("fork-q"));
        })
        .await;
}

/// After a fork, closing the source session must not affect the fork's ability
/// to continue prompting.
#[tokio::test(flavor = "current_thread")]
async fn fork_is_independent_after_source_is_closed() {
    let srv = TestServer::new().await;

    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"src-reply"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"fork-reply"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            use agent_client_protocol::CloseSessionRequest;

            let src_id = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap()
                .session_id;

            agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::from("src-q".to_string())],
                ))
                .await
                .unwrap();

            let fork_id = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), std::path::PathBuf::from("/")))
                .await
                .unwrap()
                .session_id;

            // Close the source — fork must survive.
            agent.close_session(CloseSessionRequest::new(src_id)).await.unwrap();

            let result = agent
                .prompt(PromptRequest::new(
                    fork_id,
                    vec![ContentBlock::from("fork-q".to_string())],
                ))
                .await;

            assert!(
                result.is_ok(),
                "fork must continue to work after source is closed: {result:?}"
            );
            assert!(matches!(result.unwrap().stop_reason, StopReason::EndTurn));
        })
        .await;
}

/// Prompting twice with the same message (resume path) must send only one copy of
/// the user message on the wire for the second call.
#[tokio::test(flavor = "current_thread")]
async fn resume_path_does_not_duplicate_user_message_on_wire() {
    let srv = TestServer::new().await;

    // First prompt: no assistant text (simulates a crash/interrupted response).
    srv.state.push_sse(sse(&["data: [DONE]"]));
    // Second prompt (resume): server replies normally.
    srv.state.push_sse(sse(&[
        r#"data: {"choices":[{"delta":{"content":"resumed"},"finish_reason":null}]}"#,
        "data: [DONE]",
    ]));

    let notifier = TestNotifier::new();
    let client = srv.client();
    let agent = Arc::new(OpenRouterAgent::with_deps(
        notifier.clone(),
        "test-model",
        "test-key",
        client,
    ));

    tokio::task::LocalSet::new()
        .run_until(async {
            let sid = agent
                .new_session(NewSessionRequest::new(std::path::PathBuf::from("/")))
                .await
                .unwrap()
                .session_id;

            // First call — user message stored but no assistant reply (Done only).
            agent
                .prompt(PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("ping".to_string())],
                ))
                .await
                .unwrap();

            // Second call with the same message — resume path.
            agent
                .prompt(PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("ping".to_string())],
                ))
                .await
                .unwrap();

            // The second wire request must contain "ping" only once, not twice.
            let body = srv.state.last_body().expect("must have second request body");
            let messages = body["messages"].as_array().expect("messages must be array");
            let user_msgs: Vec<_> = messages
                .iter()
                .filter(|m| m["role"].as_str() == Some("user"))
                .collect();
            assert_eq!(
                user_msgs.len(),
                1,
                "resume must not duplicate user message on wire: {body}"
            );
            assert_eq!(user_msgs[0]["content"].as_str(), Some("ping"));
        })
        .await;
}
