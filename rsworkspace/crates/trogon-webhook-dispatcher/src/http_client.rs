use std::future::Future;
use std::time::Duration;

/// Outbound HTTP POST abstraction.
///
/// The production implementation uses `reqwest`. The mock collects calls
/// in memory so unit tests can assert on what was dispatched without a live
/// HTTP server.
pub trait WebhookClient: Send + Sync + Clone + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn post(
        &self,
        url: &str,
        payload: &[u8],
        headers: Vec<(String, String)>,
    ) -> impl Future<Output = Result<u16, Self::Error>> + Send;
}

// ── Production implementation ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct ReqwestWebhookClient {
    inner: reqwest::Client,
    timeout: Duration,
}

impl ReqwestWebhookClient {
    pub fn new(timeout: Duration) -> Self {
        Self {
            inner: reqwest::Client::new(),
            timeout,
        }
    }
}

impl WebhookClient for ReqwestWebhookClient {
    type Error = reqwest::Error;

    async fn post(&self, url: &str, payload: &[u8], headers: Vec<(String, String)>) -> Result<u16, reqwest::Error> {
        let mut builder = self
            .inner
            .post(url)
            .timeout(self.timeout)
            .header("Content-Type", "application/json")
            .body(payload.to_vec());

        for (k, v) in headers {
            builder = builder.header(k, v);
        }

        let response = builder.send().await?;
        Ok(response.status().as_u16())
    }
}

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub struct RecordedCall {
        pub url: String,
        pub payload: Vec<u8>,
        pub headers: Vec<(String, String)>,
    }

    #[derive(Debug)]
    pub struct MockClientError(pub String);

    impl std::fmt::Display for MockClientError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for MockClientError {}

    #[derive(Clone, Default)]
    pub struct MockWebhookClient {
        calls: Arc<Mutex<Vec<RecordedCall>>>,
        status_code: Arc<Mutex<u16>>,
    }

    impl MockWebhookClient {
        pub fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                status_code: Arc::new(Mutex::new(200)),
            }
        }

        pub fn set_status(&self, code: u16) {
            *self.status_code.lock().unwrap() = code;
        }

        pub fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl WebhookClient for MockWebhookClient {
        type Error = MockClientError;

        async fn post(&self, url: &str, payload: &[u8], headers: Vec<(String, String)>) -> Result<u16, MockClientError> {
            self.calls.lock().unwrap().push(RecordedCall {
                url: url.to_string(),
                payload: payload.to_vec(),
                headers,
            });
            Ok(*self.status_code.lock().unwrap())
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::mock::MockWebhookClient;
    use super::WebhookClient;

    #[tokio::test]
    async fn mock_records_url_and_payload() {
        let client = MockWebhookClient::new();
        client.post("https://example.com/hook", b"hello", vec![]).await.unwrap();
        let calls = client.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://example.com/hook");
        assert_eq!(calls[0].payload, b"hello");
    }

    #[tokio::test]
    async fn mock_returns_configured_status_code() {
        let client = MockWebhookClient::new();
        client.set_status(201);
        let status = client.post("https://example.com", b"", vec![]).await.unwrap();
        assert_eq!(status, 201);
    }

    #[tokio::test]
    async fn mock_records_all_headers() {
        let client = MockWebhookClient::new();
        let headers = vec![
            ("X-Trogon-Event".to_string(), "push".to_string()),
            ("X-Trogon-Delivery".to_string(), "abc123".to_string()),
        ];
        client.post("https://example.com", b"", headers).await.unwrap();
        let calls = client.calls();
        assert_eq!(calls[0].headers.len(), 2);
        assert_eq!(calls[0].headers[0].0, "X-Trogon-Event");
    }

    #[tokio::test]
    async fn mock_accumulates_multiple_calls() {
        let client = MockWebhookClient::new();
        client.post("https://a.com", b"1", vec![]).await.unwrap();
        client.post("https://b.com", b"2", vec![]).await.unwrap();
        assert_eq!(client.calls().len(), 2);
    }

    #[tokio::test]
    async fn mock_default_status_is_200() {
        let client = MockWebhookClient::new();
        let status = client.post("https://example.com", b"", vec![]).await.unwrap();
        assert_eq!(status, 200);
    }
}
