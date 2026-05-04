//! In-memory mock implementations for unit testing without NATS or HTTP.
//!
//! Enabled with the `test-helpers` feature:
//!
//! ```toml
//! [dev-dependencies]
//! trogon-secret-proxy = { path = "...", features = ["test-helpers"] }
//! ```

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;

use crate::traits::{
    HttpClient, HttpResponse, JetStreamConsumerClient, JetStreamPublisher, JsMsg, NatsClient,
    StreamingHttpResponse,
};

// ── MockNatsClient ────────────────────────────────────────────────────────────

/// In-memory NATS client.
///
/// Pre-load messages per subject with [`seed_messages`]; capture published
/// payloads after the fact via [`published`].
#[derive(Clone, Default)]
pub struct MockNatsClient {
    /// Messages to deliver when a subject is subscribed.
    subscriptions: Arc<Mutex<HashMap<String, VecDeque<async_nats::Message>>>>,
    /// Messages delivered to the next `subscribe()` call regardless of subject.
    next_subscription: Arc<Mutex<VecDeque<async_nats::Message>>>,
    /// All (subject, payload) pairs published via this client.
    published: Arc<Mutex<Vec<(String, Bytes)>>>,
}

impl MockNatsClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-populate messages that will be delivered to a subscriber on `subject`.
    pub fn seed_messages(&self, subject: &str, messages: Vec<async_nats::Message>) {
        self.subscriptions
            .lock()
            .unwrap()
            .insert(subject.to_string(), messages.into());
    }

    /// Pre-populate a message delivered to the next `subscribe()` call,
    /// regardless of subject.  Useful when the reply subject is a dynamic UUID.
    pub fn seed_next_subscription(&self, msg: async_nats::Message) {
        self.next_subscription.lock().unwrap().push_back(msg);
    }

    /// Return all (subject, payload) pairs published so far.
    pub fn published(&self) -> Vec<(String, Bytes)> {
        self.published.lock().unwrap().clone()
    }
}

impl NatsClient for MockNatsClient {
    type Sub = MockSubscription;

    async fn subscribe(&self, subject: String) -> Result<MockSubscription, String> {
        let mut msgs: VecDeque<_> = self
            .next_subscription
            .lock()
            .unwrap()
            .drain(..)
            .collect();
        msgs.extend(
            self.subscriptions
                .lock()
                .unwrap()
                .remove(&subject)
                .unwrap_or_default(),
        );
        Ok(MockSubscription::new(msgs))
    }

    async fn publish(&self, subject: String, payload: Bytes) -> Result<(), String> {
        self.published.lock().unwrap().push((subject, payload));
        Ok(())
    }
}

/// A pre-loaded, finite stream of NATS messages.
pub struct MockSubscription {
    msgs: VecDeque<async_nats::Message>,
}

impl MockSubscription {
    fn new(msgs: VecDeque<async_nats::Message>) -> Self {
        Self { msgs }
    }
}

impl Stream for MockSubscription {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.msgs.pop_front())
    }
}

impl Unpin for MockSubscription {}

// ── MockJetStreamPublisher ────────────────────────────────────────────────────

/// Records all messages published to JetStream.
#[derive(Clone, Default)]
pub struct MockJetStreamPublisher {
    published: Arc<Mutex<Vec<PublishedMsg>>>,
}

#[derive(Debug, Clone)]
pub struct PublishedMsg {
    pub subject: String,
    pub payload: Bytes,
}

impl MockJetStreamPublisher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn published(&self) -> Vec<PublishedMsg> {
        self.published.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.published.lock().unwrap().len()
    }
}

impl JetStreamPublisher for MockJetStreamPublisher {
    async fn publish_with_headers(
        &self,
        subject: String,
        _headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<(), String> {
        self.published
            .lock()
            .unwrap()
            .push(PublishedMsg { subject, payload });
        Ok(())
    }
}

// ── MockHttpClient ────────────────────────────────────────────────────────────

/// Canned HTTP response for a specific URL prefix.
#[derive(Clone)]
pub struct MockHttpClient {
    responses: Arc<Mutex<VecDeque<HttpResponse>>>,
    /// Streaming responses: stored as (status, headers, chunks) so the struct
    /// remains Clone (StreamingHttpResponse itself is not Clone).
    streaming: Arc<Mutex<VecDeque<Result<(u16, Vec<(String, String)>, Vec<Vec<u8>>), String>>>>,
}

impl MockHttpClient {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::new())),
            streaming: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Queue a response to be returned by the next `send_request` call.
    pub fn push_response(&self, response: HttpResponse) {
        self.responses.lock().unwrap().push_back(response);
    }

    /// Queue a simple 200 OK response with a JSON body.
    pub fn push_ok(&self, body: &str) {
        self.push_response(HttpResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: body.as_bytes().to_vec(),
        });
    }

    /// Queue a 5xx error response.
    pub fn push_error(&self, status: u16, body: &str) {
        self.push_response(HttpResponse {
            status,
            headers: vec![],
            body: body.as_bytes().to_vec(),
        });
    }

    /// Queue a streaming response for the next `send_request_streaming` call.
    ///
    /// `chunks` is the sequence of byte slices the stream will yield, one at a
    /// time, in order.
    pub fn push_streaming_ok(
        &self,
        status: u16,
        headers: Vec<(String, String)>,
        chunks: Vec<Vec<u8>>,
    ) {
        self.streaming
            .lock()
            .unwrap()
            .push_back(Ok((status, headers, chunks)));
    }

    /// Queue a transport error for the next `send_request_streaming` call.
    pub fn push_streaming_error(&self, err: impl Into<String>) {
        self.streaming.lock().unwrap().push_back(Err(err.into()));
    }
}

impl Default for MockHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for MockHttpClient {
    async fn send_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(String, String)],
        _body: &[u8],
    ) -> Result<HttpResponse, String> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "MockHttpClient: no more queued responses".to_string())
    }

    async fn send_request_streaming(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(String, String)],
        _body: &[u8],
    ) -> Result<StreamingHttpResponse, String> {
        let entry = self
            .streaming
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "MockHttpClient: no more queued streaming responses".to_string())?;

        match entry {
            Ok((status, headers, chunks)) => {
                let chunk_stream = futures_util::stream::iter(
                    chunks.into_iter().map(|c| Ok::<Bytes, String>(Bytes::from(c))),
                );
                Ok(StreamingHttpResponse {
                    status,
                    headers,
                    chunks: Box::pin(chunk_stream),
                })
            }
            Err(e) => Err(e),
        }
    }
}

// ── MockJetStreamConsumerClient ───────────────────────────────────────────────

/// In-memory JetStream consumer that serves pre-loaded messages.
#[derive(Clone, Default)]
pub struct MockJetStreamConsumerClient {
    messages: Arc<Mutex<VecDeque<Result<JsMsg, String>>>>,
}

impl MockJetStreamConsumerClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a message for the next consumer.
    pub fn push_msg(&self, payload: &[u8]) {
        let js_msg = JsMsg::new(
            Bytes::copy_from_slice(payload),
            "mock.subject".to_string(),
            Box::new(|| Box::pin(async {})),
            Box::new(|| Box::pin(async {})),
        );
        self.messages.lock().unwrap().push_back(Ok(js_msg));
    }

    /// Queue an error to be returned by the stream.
    pub fn push_error(&self, err: impl Into<String>) {
        self.messages.lock().unwrap().push_back(Err(err.into()));
    }
}

impl JetStreamConsumerClient for MockJetStreamConsumerClient {
    type Messages = MockMessages;

    async fn get_messages(
        &self,
        _stream_name: &str,
        _consumer_name: &str,
    ) -> Result<MockMessages, String> {
        let msgs = std::mem::take(&mut *self.messages.lock().unwrap());
        Ok(MockMessages { msgs })
    }
}

/// A finite, pre-loaded stream of JetStream messages.
pub struct MockMessages {
    msgs: VecDeque<Result<JsMsg, String>>,
}

impl Stream for MockMessages {
    type Item = Result<JsMsg, String>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.msgs.pop_front())
    }
}

impl Unpin for MockMessages {}
