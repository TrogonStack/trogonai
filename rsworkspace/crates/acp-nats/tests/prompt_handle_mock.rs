//! Unit-style tests for `prompt::handle` using a lightweight in-memory mock.
//!
//! These tests cover error paths that require no real NATS server:
//!   - second subscribe (cancel_notify) fails  → lines 69-73
//!   - event stream closes before first message → lines 124-128
//!   - 600-second operation timeout fires       → lines 129-133

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::{AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
use agent_client_protocol::{Agent, PromptRequest};
use futures::channel::mpsc;
use futures::stream::BoxStream;
use trogon_std::time::SystemClock;

// ── minimal multi-stream mock ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MockErr(String);

impl std::fmt::Display for MockErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MockErr {}

/// A NATS mock that serves subscribe streams from a queue.
/// Each `inject()` call enqueues one stream. `subscribe()` dequeues and returns
/// the next stream, or returns `Err` when the queue is empty.
#[derive(Clone)]
struct MultiStreamMock {
    streams: Arc<Mutex<VecDeque<mpsc::UnboundedReceiver<async_nats::Message>>>>,
}

impl MultiStreamMock {
    fn new() -> Self {
        Self {
            streams: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Enqueue a new subscription stream. Returns the sender end; drop it to
    /// close the stream, or send messages into it to feed the subscriber.
    fn inject(&self) -> mpsc::UnboundedSender<async_nats::Message> {
        let (tx, rx) = mpsc::unbounded();
        self.streams.lock().unwrap().push_back(rx);
        tx
    }
}

impl trogon_nats::client::SubscribeClient for MultiStreamMock {
    type SubscribeError = MockErr;
    type Subscription = BoxStream<'static, async_nats::Message>;

    async fn subscribe<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
    ) -> Result<Self::Subscription, Self::SubscribeError> {
        match self.streams.lock().unwrap().pop_front() {
            Some(rx) => Ok(Box::pin(rx) as BoxStream<'static, async_nats::Message>),
            None => Err(MockErr(
                "mock: no stream available for subscribe".to_string(),
            )),
        }
    }
}

impl trogon_nats::client::PublishClient for MultiStreamMock {
    type PublishError = MockErr;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: bytes::Bytes,
    ) -> Result<(), Self::PublishError> {
        Ok(())
    }
}

impl trogon_nats::client::FlushClient for MultiStreamMock {
    type FlushError = MockErr;

    async fn flush(&self) -> Result<(), Self::FlushError> {
        Ok(())
    }
}

impl trogon_nats::client::RequestClient for MultiStreamMock {
    type RequestError = MockErr;

    async fn request_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: bytes::Bytes,
    ) -> Result<async_nats::Message, Self::RequestError> {
        Err(MockErr("mock: request not implemented".to_string()))
    }
}

// ── bridge builder ────────────────────────────────────────────────────────────

fn make_mock_bridge(mock: MultiStreamMock) -> Bridge<MultiStreamMock, SystemClock> {
    let config = Config::new(
        AcpPrefix::new("acp").unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    );
    let meter = opentelemetry::global::meter("prompt-handle-mock-test");
    // Drop rx immediately — notification sends during these tests will fail,
    // but we're testing the subscribe/stream/timeout paths, not notifications.
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Bridge::new(mock, SystemClock, &meter, config, tx)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// When the third `subscribe()` call (for `session_cancelled`) fails, `handle`
/// must return an `InternalError` describing the failure.
///
/// Covers: lines 69-73 in `agent/prompt.rs`
#[tokio::test]
async fn subscribe_cancel_notify_failure_returns_error() {
    let mock = MultiStreamMock::new();
    // Inject two streams → first subscribe (notifications) and second (response) succeed,
    // third subscribe (cancel) fails.
    let _notifications_tx = mock.inject();
    let _response_tx = mock.inject();

    let bridge = make_mock_bridge(mock);
    let err = bridge
        .prompt(PromptRequest::new("session-123", vec![]))
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("subscribe cancelled"),
        "expected 'subscribe cancelled' in error, got: {err}"
    );
}

/// When the event stream closes before any message arrives (sender dropped),
/// `handle` must return an `InternalError` about the stream closing.
///
/// Covers: lines 124-128 in `agent/prompt.rs`
#[tokio::test]
async fn event_stream_closed_before_message_returns_error() {
    let mock = MultiStreamMock::new();
    let notifications_tx = mock.inject(); // first subscribe → notifications stream
    let _response_tx = mock.inject(); // second subscribe → response stream (never fires)
    let _cancel_tx = mock.inject(); // third subscribe → cancel stream (never fires)

    // Drop immediately so the notifications stream is already closed when polled.
    drop(notifications_tx);

    let bridge = make_mock_bridge(mock);
    let err = bridge
        .prompt(PromptRequest::new("session-123", vec![]))
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("stream closed"),
        "expected 'stream closed' in error, got: {err}"
    );
}

/// When no event arrives within 600 seconds, `handle` must return a timeout error.
///
/// Uses `start_paused = true` + `spawn_local` so the clock can be fast-forwarded
/// without waiting real time.
///
/// Covers: lines 129-133 in `agent/prompt.rs`
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn event_stream_timeout_after_600_seconds_returns_error() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let handle = tokio::task::spawn_local(async {
                let mock = MultiStreamMock::new();
                let _notifications_tx = mock.inject(); // first subscribe → never sends (no drop → no close)
                let _response_tx = mock.inject(); // second subscribe → never sends (timeout path)
                let _cancel_tx = mock.inject(); // third subscribe → never fires
                let bridge = make_mock_bridge(mock);
                bridge
                    .prompt(PromptRequest::new("session-123", vec![]))
                    .await
            });

            // Yield to let the spawned task start and register the 600-second timer.
            tokio::task::yield_now().await;

            // Jump the clock past the 600-second prompt timeout.
            tokio::time::advance(Duration::from_secs(601)).await;

            // Yield again to let the timer fire and the task produce its result.
            tokio::task::yield_now().await;

            let result = handle.await.unwrap();
            assert!(
                result.is_err(),
                "expected Err from timeout, got: {result:?}"
            );
            assert!(
                result.unwrap_err().to_string().contains("timed out"),
                "expected 'timed out' in error message"
            );
        })
        .await;
}
