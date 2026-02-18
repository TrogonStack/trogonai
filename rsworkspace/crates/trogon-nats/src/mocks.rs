use crate::client::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use async_nats::subject::ToSubject;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct MockError(pub String);

impl std::fmt::Display for MockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MockError {}

#[derive(Clone, Debug)]
pub struct MockNatsClient {
    published: Arc<Mutex<Vec<PublishedMessage>>>,
    subscribed_subjects: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Debug)]
struct PublishedMessage {
    subject: String,
    #[allow(dead_code)]
    payload: bytes::Bytes,
}

impl MockNatsClient {
    pub fn new() -> Self {
        Self {
            published: Arc::new(Mutex::new(Vec::new())),
            subscribed_subjects: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn published_messages(&self) -> Vec<String> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.subject.clone())
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
pub struct AdvancedMockNatsClient {
    base: MockNatsClient,
    request_responses: Arc<Mutex<std::collections::HashMap<String, bytes::Bytes>>>,
    should_fail_request: Arc<Mutex<bool>>,
}

impl std::fmt::Debug for AdvancedMockNatsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdvancedMockNatsClient")
            .field("base", &self.base)
            .field(
                "request_responses",
                &format!(
                    "{} configured responses",
                    self.request_responses.lock().unwrap().len()
                ),
            )
            .field("should_fail_request", &self.should_fail_request)
            .finish()
    }
}

impl AdvancedMockNatsClient {
    pub fn new() -> Self {
        Self {
            base: MockNatsClient::new(),
            request_responses: Arc::new(Mutex::new(std::collections::HashMap::new())),
            should_fail_request: Arc::new(Mutex::new(false)),
        }
    }

    pub fn set_response(&self, subject: &str, response: bytes::Bytes) {
        self.request_responses
            .lock()
            .unwrap()
            .insert(subject.to_string(), response);
    }

    pub fn fail_next_request(&self) {
        *self.should_fail_request.lock().unwrap() = true;
    }

    pub fn published_messages(&self) -> Vec<String> {
        self.base.published_messages()
    }

    pub fn subscribed_to(&self) -> Vec<String> {
        self.base.subscribed_to()
    }

    pub fn clear_responses(&self) {
        self.request_responses.lock().unwrap().clear();
    }
}

impl Default for AdvancedMockNatsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscribeClient for MockNatsClient {
    type SubscribeError = MockError;

    async fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> Result<async_nats::Subscriber, MockError> {
        self.subscribed_subjects
            .lock()
            .unwrap()
            .push(subject.to_subject().to_string());
        Err(MockError("mock: subscribe not implemented".to_string()))
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
    type SubscribeError = MockError;

    async fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> Result<async_nats::Subscriber, MockError> {
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
        let should_fail = *self.should_fail_request.lock().unwrap();
        if should_fail {
            *self.should_fail_request.lock().unwrap() = false;
            return Err(MockError("simulated request failure".to_string()));
        }

        if let Some(response_payload) = self.request_responses.lock().unwrap().get(&subject) {
            Ok(async_nats::Message {
                subject: subject.clone().into(),
                reply: None,
                payload: response_payload.clone(),
                headers: None,
                length: response_payload.len(),
                status: None,
                description: None,
            })
        } else {
            Err(MockError(format!(
                "no response configured for subject: {}",
                subject
            )))
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
        self.base
            .publish_with_headers(subject, headers, payload)
            .await
    }
}

impl FlushClient for AdvancedMockNatsClient {
    type FlushError = MockError;

    async fn flush(&self) -> Result<(), MockError> {
        self.base.flush().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_client_default() {
        let mock = MockNatsClient::default();
        assert!(mock.published_messages().is_empty());
        assert!(mock.subscribed_to().is_empty());
    }

    #[tokio::test]
    async fn mock_client_tracks_publish() {
        let mock = MockNatsClient::new();
        let _ = mock
            .publish_with_headers(
                "foo",
                async_nats::HeaderMap::new(),
                bytes::Bytes::from("bar"),
            )
            .await;
        assert_eq!(mock.published_messages(), vec!["foo"]);
    }

    #[tokio::test]
    async fn mock_client_tracks_subscribe() {
        let mock = MockNatsClient::new();
        let _ = mock.subscribe("test.sub").await;
        assert_eq!(mock.subscribed_to(), vec!["test.sub"]);
    }

    #[test]
    fn advanced_mock_default() {
        let mock = AdvancedMockNatsClient::default();
        assert!(mock.published_messages().is_empty());
        assert!(mock.subscribed_to().is_empty());
    }

    #[test]
    fn advanced_mock_clear_responses() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a", "b".into());
        mock.clear_responses();
        assert!(mock.request_responses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn advanced_mock_request_no_response_configured() {
        let mock = AdvancedMockNatsClient::new();
        let result = mock
            .request_with_headers(
                "missing",
                async_nats::HeaderMap::new(),
                bytes::Bytes::from("x"),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().0.contains("no response configured"));
    }

    #[test]
    fn advanced_mock_debug_format() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("a", "b".into());
        let dbg = format!("{:?}", mock);
        assert!(dbg.contains("1 configured responses"));
    }

    #[test]
    fn mock_error_display() {
        let err = MockError("test".into());
        assert_eq!(err.to_string(), "test");
        assert!(std::error::Error::source(&err).is_none());
    }
}
