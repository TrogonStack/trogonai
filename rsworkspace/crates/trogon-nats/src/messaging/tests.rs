use super::*;

#[cfg(feature = "test-support")]
use crate::mocks::AdvancedMockNatsClient;

#[cfg(feature = "test-support")]
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct TestRequest {
    message: String,
}

#[cfg(feature = "test-support")]
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct TestResponse {
    result: String,
}

#[cfg(feature = "test-support")]
struct FailingSerialize;

#[cfg(feature = "test-support")]
impl serde::Serialize for FailingSerialize {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom(format!(
            "{} cannot be serialized",
            std::any::type_name::<S>()
        )))
    }
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
fn test_flush_policy_default() {
    let policy = FlushPolicy::default();
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

#[test]
fn header_map_carrier_set_inserts_value() {
    use opentelemetry::propagation::Injector;
    let mut headers = async_nats::HeaderMap::new();
    let mut carrier = HeaderMapCarrier(&mut headers);
    carrier.set("x-test-key", "test-value".to_string());
    assert_eq!(headers.get("x-test-key").map(|v| v.as_str()), Some("test-value"));
}

#[test]
fn inject_trace_context_preserves_existing_headers() {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("X-Custom", "preserved");
    inject_trace_context(&mut headers);
    assert_eq!(headers.get("X-Custom").map(|v| v.as_str()), Some("preserved"),);
}

#[tokio::test]
async fn retry_policy_execute_does_not_panic_with_high_retry_count() {
    tokio::time::pause();

    let policy = RetryPolicy {
        max_retries: 33,
        initial_retry_delay: Duration::from_millis(1),
    };

    let result = policy
        .execute(
            || async { Err(PublishOperationError("always fails".into())) },
            "test_op",
            "test.subject",
        )
        .await;

    assert!(matches!(
        result,
        Err(NatsError::PublishOperationExhausted { attempts: 34, .. })
    ));
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
async fn test_request_serialize_error() {
    let mock = AdvancedMockNatsClient::new();

    let result: Result<TestResponse, NatsError> = request(&mock, "test.subject", &FailingSerialize).await;

    assert!(matches!(result, Err(NatsError::Serialize(_))));
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
async fn test_publish_serialize_error() {
    let mock = AdvancedMockNatsClient::new();

    let result = publish(&mock, "test.subject", &FailingSerialize, PublishOptions::simple()).await;

    assert!(matches!(result, Err(NatsError::Serialize(_))));
    assert!(mock.published_messages().is_empty());
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

#[tokio::test]
#[cfg(feature = "test-support")]
async fn test_publish_returns_error_when_publish_fails() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_publish();
    let data = TestRequest {
        message: "test".to_string(),
    };

    let result = publish(&mock, "test.subject", &data, PublishOptions::simple()).await;

    assert!(matches!(result, Err(NatsError::PublishOperation(_))));
    assert!(mock.published_messages().is_empty());
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn test_publish_returns_error_when_flush_fails() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_flush();
    let data = TestRequest {
        message: "test".to_string(),
    };
    let options = PublishOptions::builder()
        .flush_policy(FlushPolicy::no_retries())
        .build();

    let result = publish(&mock, "test.subject", &data, options).await;

    assert!(matches!(result, Err(NatsError::PublishOperation(_))));
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
    assert!(err.to_string().contains("Request to 'test.subject' timed out"));
}

#[test]
fn test_default_timeout_constant() {
    assert_eq!(DEFAULT_TIMEOUT.as_secs(), 30);
}

#[test]
fn nats_error_display_all_variants() {
    let serialize_err = NatsError::Serialize(serde_json::from_str::<String>("bad").unwrap_err());
    assert!(serialize_err.to_string().contains("serialize request"));

    let deserialize_err = NatsError::Deserialize(serde_json::from_str::<String>("bad").unwrap_err());
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
    let serialize_err = NatsError::Serialize(serde_json::from_str::<String>("bad").unwrap_err());
    assert!(std::error::Error::source(&serialize_err).is_some());

    let deserialize_err = NatsError::Deserialize(serde_json::from_str::<String>("bad").unwrap_err());
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

    let timeout = NatsError::Timeout { subject: "s".into() };
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

    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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

    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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
        NatsError::PublishOperationExhausted { attempts, subject, .. } => {
            assert_eq!(attempts, 4);
            assert_eq!(subject, "test.subject");
        }
        e => panic!("Expected PublishOperationExhausted error, got: {:?}", e),
    }
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn test_request_with_timeout_returns_timeout_error() {
    let mock = AdvancedMockNatsClient::new();
    mock.hang_next_request();

    let req = TestRequest {
        message: "hello".to_string(),
    };

    let result: Result<TestResponse, NatsError> =
        request_with_timeout(&mock, "test.subject", &req, Duration::from_millis(1)).await;

    assert!(matches!(result, Err(NatsError::Timeout { .. })));
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
