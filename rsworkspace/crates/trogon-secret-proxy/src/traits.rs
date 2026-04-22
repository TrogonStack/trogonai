//! Trait abstractions for every external dependency.
//!
//! All concrete types live in [`crate::impls`]; mocks live in [`crate::mocks`]
//! (feature-gated behind `test-helpers`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use bytes::Bytes;
use futures_util::Stream;

// ── NatsClient ────────────────────────────────────────────────────────────────

/// Core NATS subscribe / publish operations.
///
/// The associated `Sub` type must be `Unpin` so callers can poll it with
/// `StreamExt::next()` in `tokio::select!` branches without pinning gymnastics.
pub trait NatsClient: Clone + Send + Sync + 'static {
    type Sub: Stream<Item = async_nats::Message> + Send + Unpin + 'static;

    fn subscribe(&self, subject: String) -> impl Future<Output = Result<Self::Sub, String>> + Send;

    fn publish(
        &self,
        subject: String,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

// ── JetStreamPublisher ────────────────────────────────────────────────────────

/// Publish messages with NATS headers into a JetStream stream (proxy-side).
pub trait JetStreamPublisher: Clone + Send + Sync + 'static {
    fn publish_with_headers(
        &self,
        subject: String,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

// ── HttpClient ────────────────────────────────────────────────────────────────

/// Raw HTTP response from an upstream AI provider.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Forward HTTP requests to upstream AI providers (worker-side).
pub trait HttpClient: Clone + Send + Sync + 'static {
    fn send_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> impl Future<Output = Result<HttpResponse, String>> + Send;
}

// ── JetStreamConsumerClient ───────────────────────────────────────────────────

/// A one-shot async function used for ack / nack callbacks.
type AckFn = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A JetStream message with payload, subject, and fire-once ack / nack.
///
/// Both `ack()` and `nack()` are idempotent: calling either after the first
/// invocation is a no-op.
pub struct JsMsg {
    pub payload: Bytes,
    pub subject: String,
    ack: Mutex<Option<AckFn>>,
    nack: Mutex<Option<AckFn>>,
}

impl JsMsg {
    pub fn new(payload: Bytes, subject: String, ack: AckFn, nack: AckFn) -> Self {
        Self {
            payload,
            subject,
            ack: Mutex::new(Some(ack)),
            nack: Mutex::new(Some(nack)),
        }
    }

    pub async fn ack(&self) {
        let f = self.ack.lock().unwrap().take();
        if let Some(f) = f {
            f().await;
        }
    }

    pub async fn nack(&self) {
        let f = self.nack.lock().unwrap().take();
        if let Some(f) = f {
            f().await;
        }
    }
}

// SAFETY: AckFn is Send (Box<dyn FnOnce + Send>), and Mutex<Option<AckFn>>
// is Send + Sync as long as AckFn is Send — which our type alias guarantees.
// The `Mutex` ensures exclusive access across threads.
unsafe impl Send for JsMsg {}
unsafe impl Sync for JsMsg {}

/// Pull a stream of messages from a JetStream durable consumer (worker-side).
///
/// The associated `Messages` type must be `Unpin` so the worker loop can call
/// `messages.next()` directly inside `while let`.
pub trait JetStreamConsumerClient: Clone + Send + Sync + 'static {
    type Messages: Stream<Item = Result<JsMsg, String>> + Send + Unpin + 'static;

    fn get_messages(
        &self,
        stream_name: &str,
        consumer_name: &str,
    ) -> impl Future<Output = Result<Self::Messages, String>> + Send;
}
