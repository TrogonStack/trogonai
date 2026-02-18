use crate::client::{FlushClient, PublishClient, RequestClient};
use async_nats::header::HeaderMap;
use opentelemetry::propagation::Injector;
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

struct HeaderMapCarrier<'a>(&'a mut HeaderMap);

impl Injector for HeaderMapCarrier<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key, value.as_str());
    }
}

pub fn inject_trace_context(headers: &mut HeaderMap) {
    let cx = Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderMapCarrier(headers));
    });
}

pub fn headers_with_trace_context() -> HeaderMap {
    let mut headers = HeaderMap::new();
    inject_trace_context(&mut headers);
    headers
}

pub async fn request_with_timeout<N: RequestClient, Req, Res>(
    client: &N,
    subject: &str,
    request: &Req,
    timeout: Duration,
) -> Result<Res, NatsError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    let payload = serde_json::to_vec(request).map_err(NatsError::Serialize)?;
    let headers = headers_with_trace_context();

    let response = tokio::time::timeout(
        timeout,
        client.request_with_headers(subject.to_string(), headers, payload.into()),
    )
    .await
    .map_err(|_| NatsError::Timeout {
        subject: subject.to_string(),
    })?
    .map_err(|e| NatsError::Request {
        subject: subject.to_string(),
        error: e.to_string(),
    })?;

    let payload_str = String::from_utf8_lossy(&response.payload);
    tracing::debug!(payload = %payload_str, "Received NATS response");

    serde_json::from_slice(&response.payload).map_err(|e| {
        tracing::error!(payload = %payload_str, error = %e, "Failed to deserialize NATS response");
        NatsError::Deserialize(e)
    })
}

pub async fn request<N: RequestClient, Req, Res>(
    client: &N,
    subject: &str,
    request: &Req,
) -> Result<Res, NatsError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    request_with_timeout(client, subject, request, DEFAULT_TIMEOUT).await
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Set to 0 to disable retries.
    pub max_retries: u32,
    /// Exponential backoff: delay * 2^retry_number.
    pub initial_retry_delay: Duration,
}

impl RetryPolicy {
    pub fn no_retries() -> Self {
        Self {
            max_retries: 0,
            initial_retry_delay: Duration::from_millis(50),
        }
    }

    pub fn standard() -> Self {
        Self {
            max_retries: 3,
            initial_retry_delay: Duration::from_millis(50),
        }
    }

    pub async fn execute<F, Fut>(
        &self,
        mut operation: F,
        operation_name: &str,
        subject: &str,
    ) -> Result<(), NatsError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<(), PublishOperationError>>,
    {
        let mut attempts = 0;
        let mut last_error: Option<PublishOperationError> = None;

        while attempts <= self.max_retries {
            attempts += 1;
            match operation().await {
                Ok(()) => {
                    if attempts > 1 {
                        tracing::info!(
                            operation = operation_name,
                            subject = %subject,
                            attempts,
                            "Operation succeeded after retries"
                        );
                    }
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempts <= self.max_retries {
                        let exp = (attempts - 1).min(31);
                        let delay = self.initial_retry_delay * (1u32 << exp);
                        tracing::debug!(
                            error = %last_error.as_ref().unwrap(),
                            operation = operation_name,
                            subject = %subject,
                            attempt = attempts,
                            max_retries = self.max_retries,
                            delay_ms = delay.as_millis(),
                            "Operation failed, retrying"
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        let final_error = last_error.unwrap_or_else(|| {
            PublishOperationError("No error recorded in retry loop".to_string())
        });
        if self.max_retries > 0 {
            tracing::warn!(
                error = %final_error,
                operation = operation_name,
                subject = %subject,
                total_attempts = attempts,
                "Operation failed after all retry attempts"
            );
            Err(NatsError::PublishOperationExhausted {
                error: final_error,
                subject: subject.to_string(),
                attempts,
            })
        } else {
            tracing::warn!(
                error = %final_error,
                operation = operation_name,
                subject = %subject,
                "Operation failed"
            );
            Err(NatsError::PublishOperation(final_error))
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::no_retries()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlushPolicy {
    pub retry_policy: RetryPolicy,
}

impl FlushPolicy {
    pub fn no_retries() -> Self {
        Self {
            retry_policy: RetryPolicy::no_retries(),
        }
    }

    pub fn standard() -> Self {
        Self {
            retry_policy: RetryPolicy::standard(),
        }
    }
}

impl Default for FlushPolicy {
    fn default() -> Self {
        Self::no_retries()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions {
    pub publish_retry_policy: RetryPolicy,
    pub flush: Option<FlushPolicy>,
}

impl PublishOptions {
    pub fn simple() -> Self {
        Self::default()
    }

    pub fn builder() -> PublishOptionsBuilder {
        PublishOptionsBuilder::default()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptionsBuilder {
    publish_retry_policy: RetryPolicy,
    flush: Option<FlushPolicy>,
}

impl PublishOptionsBuilder {
    pub fn publish_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.publish_retry_policy = policy;
        self
    }

    pub fn flush_policy(mut self, policy: FlushPolicy) -> Self {
        self.flush = Some(policy);
        self
    }

    pub fn build(self) -> PublishOptions {
        PublishOptions {
            publish_retry_policy: self.publish_retry_policy,
            flush: self.flush,
        }
    }
}

pub async fn publish<N: PublishClient + FlushClient, Req>(
    client: &N,
    subject: &str,
    request: &Req,
    options: PublishOptions,
) -> Result<(), NatsError>
where
    Req: Serialize,
{
    let payload = serde_json::to_vec(request).map_err(NatsError::Serialize)?;
    let headers = headers_with_trace_context();

    options
        .publish_retry_policy
        .execute(
            || async {
                client
                    .publish_with_headers(
                        subject.to_string(),
                        headers.clone(),
                        payload.clone().into(),
                    )
                    .await
                    .map_err(|e| PublishOperationError(e.to_string()))
            },
            "publish",
            subject,
        )
        .await?;

    let Some(flush_policy) = options.flush else {
        return Ok(());
    };

    flush_policy
        .retry_policy
        .execute(
            || {
                let client = client.clone();
                Box::pin(async move {
                    client
                        .flush()
                        .await
                        .map_err(|e| PublishOperationError(e.to_string()))
                })
            },
            "flush",
            subject,
        )
        .await
}

#[derive(Debug)]
pub struct PublishOperationError(pub String);

impl std::fmt::Display for PublishOperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PublishOperationError {}

#[derive(Debug)]
pub enum NatsError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    Request {
        subject: String,
        error: String,
    },
    PublishOperation(PublishOperationError),
    PublishOperationExhausted {
        error: PublishOperationError,
        subject: String,
        attempts: u32,
    },
    Timeout {
        subject: String,
    },
    Other(String),
}

impl std::fmt::Display for NatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "Failed to serialize request: {}", e),
            Self::Deserialize(e) => write!(f, "Failed to deserialize response: {}", e),
            Self::Request { subject, error } => {
                write!(f, "Request to '{}' failed: {}", subject, error)
            }
            Self::PublishOperation(e) => write!(f, "Publish operation failed: {}", e),
            Self::PublishOperationExhausted {
                error,
                subject,
                attempts,
            } => write!(
                f,
                "Publish operation failed after {} attempts on '{}': {}",
                attempts, subject, error
            ),
            Self::Timeout { subject } => write!(
                f,
                "Request to '{}' timed out. The backend may be overloaded or unresponsive.",
                subject
            ),
            Self::Other(msg) => write!(f, "NATS error: {}", msg),
        }
    }
}

impl std::error::Error for NatsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) => Some(e),
            Self::Deserialize(e) => Some(e),
            Self::PublishOperation(e) => Some(e),
            Self::PublishOperationExhausted { error, .. } => Some(error),
            Self::Request { .. } | Self::Timeout { .. } | Self::Other(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[cfg(feature = "test-support")]
    use crate::mocks::AdvancedMockNatsClient;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestRequest {
        message: String,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestResponse {
        result: String,
    }

    #[test]
    fn test_retry_policy_no_retries() {
        let policy = RetryPolicy::no_retries();
        assert_eq!(policy.max_retries, 0);
        assert_eq!(policy.initial_retry_delay.as_millis(), 50);
    }

    #[test]
    fn test_retry_policy_standard() {
        let policy = RetryPolicy::standard();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_retry_delay.as_millis(), 50);
    }

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 0);
    }

    #[test]
    fn test_flush_policy_no_retries() {
        let policy = FlushPolicy::no_retries();
        assert_eq!(policy.retry_policy.max_retries, 0);
    }

    #[test]
    fn test_flush_policy_standard() {
        let policy = FlushPolicy::standard();
        assert_eq!(policy.retry_policy.max_retries, 3);
    }

    #[test]
    fn test_publish_options_default() {
        let options = PublishOptions::default();
        assert_eq!(options.publish_retry_policy.max_retries, 0);
        assert!(options.flush.is_none());
    }

    #[test]
    fn test_publish_options_simple() {
        let options = PublishOptions::simple();
        assert_eq!(options.publish_retry_policy.max_retries, 0);
        assert!(options.flush.is_none());
    }

    #[test]
    fn test_publish_options_builder() {
        let options = PublishOptions::builder()
            .publish_retry_policy(RetryPolicy::standard())
            .flush_policy(FlushPolicy::standard())
            .build();

        assert_eq!(options.publish_retry_policy.max_retries, 3);
        assert!(options.flush.is_some());
        assert_eq!(options.flush.unwrap().retry_policy.max_retries, 3);
    }

    #[test]
    fn test_publish_options_builder_partial() {
        let options = PublishOptions::builder()
            .publish_retry_policy(RetryPolicy::standard())
            .build();

        assert_eq!(options.publish_retry_policy.max_retries, 3);
        assert!(options.flush.is_none());
    }

    #[test]
    fn test_headers_with_trace_context_creates_headermap() {
        let headers = headers_with_trace_context();
        let _ = headers.len();
    }

    #[test]
    fn test_inject_trace_context_does_not_panic() {
        let mut headers = async_nats::HeaderMap::new();
        inject_trace_context(&mut headers);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_request_with_mock_success() {
        let mock = AdvancedMockNatsClient::new();
        let response = TestResponse {
            result: "success".to_string(),
        };
        let response_bytes = serde_json::to_vec(&response).unwrap();
        mock.set_response("test.subject", response_bytes.into());

        let req = TestRequest {
            message: "hello".to_string(),
        };

        let result: Result<TestResponse, NatsError> = request(&mock, "test.subject", &req).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), response);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_request_with_timeout_custom_duration() {
        let mock = AdvancedMockNatsClient::new();
        let response = TestResponse {
            result: "success".to_string(),
        };
        let response_bytes = serde_json::to_vec(&response).unwrap();
        mock.set_response("test.subject", response_bytes.into());

        let req = TestRequest {
            message: "hello".to_string(),
        };

        let result: Result<TestResponse, NatsError> =
            request_with_timeout(&mock, "test.subject", &req, Duration::from_secs(5)).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), response);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_request_deserialize_error() {
        let mock = AdvancedMockNatsClient::new();
        mock.set_response("test.subject", "not json".into());

        let req = TestRequest {
            message: "hello".to_string(),
        };

        let result: Result<TestResponse, NatsError> = request(&mock, "test.subject", &req).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            NatsError::Deserialize(_) => {}
            e => panic!("Expected Deserialize error, got: {:?}", e),
        }
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_publish_simple() {
        let mock = AdvancedMockNatsClient::new();
        let data = TestRequest {
            message: "test".to_string(),
        };

        let result = publish(&mock, "test.subject", &data, PublishOptions::simple()).await;

        assert!(result.is_ok());
        assert_eq!(mock.published_messages(), vec!["test.subject"]);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_publish_with_flush() {
        let mock = AdvancedMockNatsClient::new();
        let data = TestRequest {
            message: "test".to_string(),
        };

        let options = PublishOptions::builder()
            .flush_policy(FlushPolicy::no_retries())
            .build();

        let result = publish(&mock, "test.subject", &data, options).await;

        assert!(result.is_ok());
        assert_eq!(mock.published_messages(), vec!["test.subject"]);
    }

    #[test]
    fn test_publish_operation_error_display() {
        let err = PublishOperationError("test error".to_string());
        assert_eq!(err.to_string(), "test error");
    }

    #[test]
    fn test_nats_error_display_timeout() {
        let err = NatsError::Timeout {
            subject: "test.subject".to_string(),
        };
        assert!(
            err.to_string()
                .contains("Request to 'test.subject' timed out")
        );
    }

    #[test]
    fn test_default_timeout_constant() {
        assert_eq!(DEFAULT_TIMEOUT.as_secs(), 30);
    }

    #[test]
    fn nats_error_display_all_variants() {
        let serialize_err =
            NatsError::Serialize(serde_json::from_str::<String>("bad").unwrap_err());
        assert!(serialize_err.to_string().contains("serialize request"));

        let deserialize_err =
            NatsError::Deserialize(serde_json::from_str::<String>("bad").unwrap_err());
        assert!(deserialize_err.to_string().contains("deserialize response"));

        let request_err = NatsError::Request {
            subject: "s".into(),
            error: "boom".into(),
        };
        assert!(request_err.to_string().contains("'s' failed: boom"));

        let pub_err = NatsError::PublishOperation(PublishOperationError("fail".into()));
        assert!(pub_err.to_string().contains("Publish operation failed"));

        let exhausted = NatsError::PublishOperationExhausted {
            error: PublishOperationError("fail".into()),
            subject: "s".into(),
            attempts: 4,
        };
        assert!(exhausted.to_string().contains("4 attempts"));

        let other = NatsError::Other("misc".into());
        assert!(other.to_string().contains("misc"));
    }

    #[test]
    fn nats_error_source() {
        let serialize_err =
            NatsError::Serialize(serde_json::from_str::<String>("bad").unwrap_err());
        assert!(std::error::Error::source(&serialize_err).is_some());

        let deserialize_err =
            NatsError::Deserialize(serde_json::from_str::<String>("bad").unwrap_err());
        assert!(std::error::Error::source(&deserialize_err).is_some());

        let pub_err = NatsError::PublishOperation(PublishOperationError("f".into()));
        assert!(std::error::Error::source(&pub_err).is_some());

        let exhausted = NatsError::PublishOperationExhausted {
            error: PublishOperationError("f".into()),
            subject: "s".into(),
            attempts: 1,
        };
        assert!(std::error::Error::source(&exhausted).is_some());

        let request_err = NatsError::Request {
            subject: "s".into(),
            error: "e".into(),
        };
        assert!(std::error::Error::source(&request_err).is_none());

        let timeout = NatsError::Timeout {
            subject: "s".into(),
        };
        assert!(std::error::Error::source(&timeout).is_none());

        let other = NatsError::Other("x".into());
        assert!(std::error::Error::source(&other).is_none());
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_retry_policy_execute_success_first_attempt() {
        use std::sync::{Arc, Mutex};

        let policy = RetryPolicy::standard();
        let call_count = Arc::new(Mutex::new(0));

        let result = {
            let count = Arc::clone(&call_count);
            policy
                .execute(
                    move || {
                        let count = Arc::clone(&count);
                        async move {
                            *count.lock().unwrap() += 1;
                            Ok(())
                        }
                    },
                    "test_operation",
                    "test.subject",
                )
                .await
        };

        assert!(result.is_ok());
        assert_eq!(*call_count.lock().unwrap(), 1);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_retry_policy_execute_success_after_retries() {
        use std::sync::{Arc, Mutex};

        let policy = RetryPolicy::standard();
        let call_count = Arc::new(Mutex::new(0));

        let result = {
            let count = Arc::clone(&call_count);
            policy
                .execute(
                    move || {
                        let count = Arc::clone(&count);
                        async move {
                            let mut c = count.lock().unwrap();
                            *c += 1;
                            if *c < 3 {
                                Err(PublishOperationError("temporary error".to_string()))
                            } else {
                                Ok(())
                            }
                        }
                    },
                    "test_operation",
                    "test.subject",
                )
                .await
        };

        assert!(result.is_ok());
        assert_eq!(*call_count.lock().unwrap(), 3);
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_retry_policy_execute_exhausted() {
        use std::sync::{Arc, Mutex};

        let policy = RetryPolicy::standard();
        let call_count = Arc::new(Mutex::new(0));

        let result = {
            let count = Arc::clone(&call_count);
            policy
                .execute(
                    move || {
                        let count = Arc::clone(&count);
                        async move {
                            *count.lock().unwrap() += 1;
                            Err(PublishOperationError("persistent error".to_string()))
                        }
                    },
                    "test_operation",
                    "test.subject",
                )
                .await
        };

        assert!(result.is_err());
        assert_eq!(*call_count.lock().unwrap(), 4); // initial + 3 retries

        match result.unwrap_err() {
            NatsError::PublishOperationExhausted {
                attempts, subject, ..
            } => {
                assert_eq!(attempts, 4);
                assert_eq!(subject, "test.subject");
            }
            e => panic!("Expected PublishOperationExhausted error, got: {:?}", e),
        }
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_request_returns_error_on_mock_failure() {
        let mock = AdvancedMockNatsClient::new();
        mock.fail_next_request();

        let req = TestRequest {
            message: "hello".to_string(),
        };

        let result: Result<TestResponse, NatsError> = request(&mock, "test.subject", &req).await;

        assert!(matches!(result, Err(NatsError::Request { .. })));
    }

    #[tokio::test]
    #[cfg(feature = "test-support")]
    async fn test_retry_policy_no_retries_fails_immediately() {
        use std::sync::{Arc, Mutex};

        let policy = RetryPolicy::no_retries();
        let call_count = Arc::new(Mutex::new(0));

        let result = {
            let count = Arc::clone(&call_count);
            policy
                .execute(
                    move || {
                        let count = Arc::clone(&count);
                        async move {
                            *count.lock().unwrap() += 1;
                            Err(PublishOperationError("error".to_string()))
                        }
                    },
                    "test_operation",
                    "test.subject",
                )
                .await
        };

        assert!(result.is_err());
        assert_eq!(*call_count.lock().unwrap(), 1); // no retries

        match result.unwrap_err() {
            NatsError::PublishOperation(_) => {}
            e => panic!("Expected PublishOperation error, got: {:?}", e),
        }
    }
}
