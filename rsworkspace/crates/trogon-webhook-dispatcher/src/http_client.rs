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
