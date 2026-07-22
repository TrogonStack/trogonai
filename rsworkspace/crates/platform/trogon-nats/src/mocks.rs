use crate::client::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use async_nats::client::{SubscribeError, SubscribeErrorKind};
use async_nats::subject::ToSubject;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::stream::BoxStream;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct MockError(pub String);

#[derive(Clone, Debug)]
pub struct MockNatsClient {
    published: Arc<Mutex<Vec<PublishedMessage>>>,
    subscribed_subjects: Arc<Mutex<Vec<String>>>,
    subscribe_streams: Arc<Mutex<std::collections::VecDeque<mpsc::UnboundedReceiver<async_nats::Message>>>>,
}

#[derive(Clone, Debug)]
struct PublishedMessage {
    subject: String,
    payload: bytes::Bytes,
    headers: async_nats::HeaderMap,
}

impl MockNatsClient {
    pub fn new() -> Self {
        Self {
            published: Arc::new(Mutex::new(Vec::new())),
            subscribed_subjects: Arc::new(Mutex::new(Vec::new())),
            subscribe_streams: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub fn inject_messages(&self) -> mpsc::UnboundedSender<async_nats::Message> {
        let (tx, rx) = mpsc::unbounded();
        self.subscribe_streams.lock().unwrap().push_back(rx);
        tx
    }

    pub fn published_messages(&self) -> Vec<String> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.subject.clone())
            .collect()
    }

    pub fn published_payloads(&self) -> Vec<bytes::Bytes> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.payload.clone())
            .collect()
    }

    pub fn published_headers(&self) -> Vec<async_nats::HeaderMap> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.headers.clone())
            .collect()
    }

    pub fn subscribed_to(&self) -> Vec<String> {
        self.subscribed_subjects.lock().unwrap().clone()
    }
}

impl Default for MockNatsClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct MockResponse {
    headers: async_nats::HeaderMap,
    payload: bytes::Bytes,
}

#[derive(Clone)]
pub struct AdvancedMockNatsClient {
    base: MockNatsClient,
    request_responses: Arc<Mutex<std::collections::HashMap<String, MockResponse>>>,
    should_fail_request: Arc<Mutex<bool>>,
    should_hang_request: Arc<Mutex<bool>>,
    should_hang_publish: Arc<Mutex<bool>>,
    publish_fail_count: Arc<Mutex<u32>>,
    flush_fail_count: Arc<Mutex<u32>>,
}

impl std::fmt::Debug for AdvancedMockNatsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdvancedMockNatsClient")
            .field("base", &self.base)
            .field(
                "request_responses",
                &format!("{} configured responses", self.request_responses.lock().unwrap().len()),
            )
            .field("should_fail_request", &self.should_fail_request)
            .field("should_hang_request", &self.should_hang_request)
            .field("should_hang_publish", &self.should_hang_publish)
            .field("publish_fail_count", &self.publish_fail_count)
            .field("flush_fail_count", &self.flush_fail_count)
            .finish()
    }
}

impl AdvancedMockNatsClient {
    pub fn new() -> Self {
        Self {
            base: MockNatsClient::new(),
            request_responses: Arc::new(Mutex::new(std::collections::HashMap::new())),
            should_fail_request: Arc::new(Mutex::new(false)),
            should_hang_request: Arc::new(Mutex::new(false)),
            should_hang_publish: Arc::new(Mutex::new(false)),
            publish_fail_count: Arc::new(Mutex::new(0)),
            flush_fail_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn fail_next_flush(&self) {
        *self.flush_fail_count.lock().unwrap() = 1;
    }

    pub fn fail_next_publish(&self) {
        *self.publish_fail_count.lock().unwrap() = 1;
    }

    /// Fail the next `n` publish attempts. Use 4 to exhaust standard retry (1 + 3 retries).
    pub fn fail_publish_count(&self, n: u32) {
        *self.publish_fail_count.lock().unwrap() = n;
    }

    pub fn set_response(&self, subject: &str, response: bytes::Bytes) {
        self.set_response_wire(subject, async_nats::HeaderMap::new(), response);
    }

    pub fn set_response_wire(&self, subject: &str, headers: async_nats::HeaderMap, response: bytes::Bytes) {
        self.request_responses.lock().unwrap().insert(
            subject.to_string(),
            MockResponse {
                headers,
                payload: response,
            },
        );
    }

    pub fn fail_next_request(&self) {
        *self.should_fail_request.lock().unwrap() = true;
    }

    pub fn hang_next_request(&self) {
        *self.should_hang_request.lock().unwrap() = true;
    }

    pub fn hang_next_publish(&self) {
        *self.should_hang_publish.lock().unwrap() = true;
    }

    pub fn published_messages(&self) -> Vec<String> {
        self.base.published_messages()
    }

    pub fn published_payloads(&self) -> Vec<bytes::Bytes> {
        self.base.published_payloads()
    }

    pub fn published_headers(&self) -> Vec<async_nats::HeaderMap> {
        self.base.published_headers()
    }

    pub fn subscribed_to(&self) -> Vec<String> {
        self.base.subscribed_to()
    }

    pub fn clear_responses(&self) {
        self.request_responses.lock().unwrap().clear();
    }

    pub fn inject_messages(&self) -> futures::channel::mpsc::UnboundedSender<async_nats::Message> {
        self.base.inject_messages()
    }
}

impl Default for AdvancedMockNatsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscribeClient for MockNatsClient {
    type SubscribeError = SubscribeError;
    type Subscription = BoxStream<'static, async_nats::Message>;

    async fn subscribe<S: ToSubject + Send>(&self, subject: S) -> Result<Self::Subscription, SubscribeError> {
        self.subscribed_subjects
            .lock()
            .unwrap()
            .push(subject.to_subject().to_string());

        match self.subscribe_streams.lock().unwrap().pop_front() {
            Some(rx) => Ok(rx.boxed()),
            None => Err(SubscribeError::with_source(
                SubscribeErrorKind::Other,
                MockError("mock: subscribe not implemented".to_string()),
            )),
        }
    }
}

impl RequestClient for MockNatsClient {
    type RequestError = MockError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: bytes::Bytes,
    ) -> Result<async_nats::Message, MockError> {
        Err(MockError("mock: request not implemented".to_string()))
    }
}

impl PublishClient for MockNatsClient {
    type PublishError = MockError;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        _headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<(), MockError> {
        self.published.lock().unwrap().push(PublishedMessage {
            subject: subject.to_subject().to_string(),
            payload,
            headers: _headers,
        });
        Ok(())
    }
}

impl FlushClient for MockNatsClient {
    type FlushError = MockError;

    async fn flush(&self) -> Result<(), MockError> {
        Ok(())
    }
}

impl SubscribeClient for AdvancedMockNatsClient {
    type SubscribeError = SubscribeError;
    type Subscription = BoxStream<'static, async_nats::Message>;

    async fn subscribe<S: ToSubject + Send>(&self, subject: S) -> Result<Self::Subscription, SubscribeError> {
        self.base.subscribe(subject).await
    }
}

impl RequestClient for AdvancedMockNatsClient {
    type RequestError = MockError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        _headers: async_nats::HeaderMap,
        _payload: bytes::Bytes,
    ) -> Result<async_nats::Message, MockError> {
        let subject = subject.to_subject().to_string();
        let should_hang = *self.should_hang_request.lock().unwrap();
        if should_hang {
            *self.should_hang_request.lock().unwrap() = false;
            std::future::pending::<()>().await;
        }
        let should_fail = *self.should_fail_request.lock().unwrap();
        if should_fail {
            *self.should_fail_request.lock().unwrap() = false;
            return Err(MockError("simulated request failure".to_string()));
        }

        if let Some(response) = self.request_responses.lock().unwrap().get(&subject) {
            Ok(async_nats::Message {
                subject: subject.clone().into(),
                reply: None,
                payload: response.payload.clone(),
                headers: Some(response.headers.clone()),
                length: response.payload.len(),
                status: None,
                description: None,
            })
        } else {
            Err(MockError(format!("no response configured for subject: {}", subject)))
        }
    }
}

impl PublishClient for AdvancedMockNatsClient {
    type PublishError = MockError;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<(), MockError> {
        let should_hang = {
            let mut should_hang = self.should_hang_publish.lock().unwrap();
            if *should_hang {
                *should_hang = false;
                true
            } else {
                false
            }
        };
        if should_hang {
            std::future::pending::<()>().await;
        }
        let should_fail = {
            let mut count = self.publish_fail_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated publish failure".to_string()));
        }
        self.base.publish_with_headers(subject, headers, payload).await
    }
}

impl FlushClient for AdvancedMockNatsClient {
    type FlushError = MockError;

    async fn flush(&self) -> Result<(), MockError> {
        let should_fail = {
            let mut count = self.flush_fail_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated flush failure".to_string()));
        }
        self.base.flush().await
    }
}

#[cfg(test)]
mod tests;
