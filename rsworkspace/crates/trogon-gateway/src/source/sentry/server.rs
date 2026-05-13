use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use super::config::SentryConfig;
use super::constants::{
    HEADER_REQUEST_ID, HEADER_RESOURCE, HEADER_SIGNATURE, HEADER_TIMESTAMP, HTTP_BODY_SIZE_MAX, NATS_HEADER_ACTION,
    NATS_HEADER_REQUEST_ID, NATS_HEADER_RESOURCE, NATS_HEADER_TIMESTAMP,
};
use super::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use serde::Deserialize;
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

#[derive(Deserialize)]
struct WebhookEnvelope {
    action: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SentryResource {
    Installation,
    EventAlert,
    Issue,
    MetricAlert,
    Error,
    Comment,
    Seer,
}

impl SentryResource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Installation => "installation",
            Self::EventAlert => "event_alert",
            Self::Issue => "issue",
            Self::MetricAlert => "metric_alert",
            Self::Error => "error",
            Self::Comment => "comment",
            Self::Seer => "seer",
        }
    }
}

#[derive(Debug)]
struct InvalidSentryResource;

impl fmt::Display for InvalidSentryResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unsupported Sentry webhook resource")
    }
}

impl std::error::Error for InvalidSentryResource {}

impl FromStr for SentryResource {
    type Err = InvalidSentryResource;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "installation" => Ok(Self::Installation),
            "event_alert" => Ok(Self::EventAlert),
            "issue" => Ok(Self::Issue),
            "metric_alert" => Ok(Self::MetricAlert),
            "error" => Ok(Self::Error),
            "comment" => Ok(Self::Comment),
            "seer" => Ok(Self::Seer),
            _ => Err(InvalidSentryResource),
        }
    }
}

fn header_value<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Sentry event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("sentry");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    client_secret: super::sentry_client_secret::SentryClientSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &SentryConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.as_str().to_owned(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age.into(),
        ..Default::default()
    })
    .await?;

    let stream = config.stream_name.as_str();
    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    info!(stream, max_age_secs, "JetStream stream ready");
    Ok(())
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &SentryConfig,
) -> Router {
    let state = AppState {
        publisher,
        client_secret: config.client_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[instrument(
    name = "sentry.webhook",
    skip_all,
    fields(
        resource = tracing::field::Empty,
        action = tracing::field::Empty,
        request_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(signature) = header_value(&headers, HEADER_SIGNATURE) else {
        warn!("Missing Sentry-Hook-Signature header");
        return StatusCode::UNAUTHORIZED;
    };

    if let Err(error) = signature::verify(state.client_secret.as_str(), &body, signature) {
        warn!(reason = %error, "Sentry webhook signature validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let Some(resource) = header_value(&headers, HEADER_RESOURCE) else {
        warn!("Missing Sentry-Hook-Resource header");
        return StatusCode::BAD_REQUEST;
    };

    let Some(timestamp) = header_value(&headers, HEADER_TIMESTAMP).map(str::to_owned) else {
        warn!("Missing Sentry-Hook-Timestamp header");
        return StatusCode::BAD_REQUEST;
    };
    let Some(request_id) = header_value(&headers, HEADER_REQUEST_ID).map(str::to_owned) else {
        warn!("Missing Request-ID header");
        return StatusCode::BAD_REQUEST;
    };

    let payload = match serde_json::from_slice::<WebhookEnvelope>(&body) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(error = %error, "Failed to parse Sentry webhook payload");
            return StatusCode::BAD_REQUEST;
        }
    };

    let resource = match resource.parse::<SentryResource>() {
        Ok(resource) => resource,
        Err(error) => {
            warn!(reason = %error, resource, "Unsupported Sentry webhook resource");
            return StatusCode::BAD_REQUEST;
        }
    };
    let action_token = match NatsToken::new(payload.action.as_str()) {
        Ok(token) => token,
        Err(error) => {
            warn!(reason = ?error, action = %payload.action, "Invalid Sentry action token");
            return StatusCode::BAD_REQUEST;
        }
    };

    let subject = format!("{}.{}.{}", state.subject_prefix, resource.as_str(), action_token);
    let span = tracing::Span::current();
    span.record("resource", resource.as_str());
    span.record("action", &payload.action);
    span.record("request_id", &request_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_RESOURCE, resource.as_str());
    nats_headers.insert(NATS_HEADER_ACTION, payload.action.as_str());
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, request_id.as_str());
    nats_headers.insert(NATS_HEADER_REQUEST_ID, request_id.as_str());
    nats_headers.insert(NATS_HEADER_TIMESTAMP, timestamp.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests {
    use super::super::config::SentryConfig;
    use super::super::constants::{
        HEADER_REQUEST_ID, HEADER_RESOURCE, HEADER_SIGNATURE, HEADER_TIMESTAMP, NATS_HEADER_ACTION,
        NATS_HEADER_REQUEST_ID, NATS_HEADER_RESOURCE, NATS_HEADER_TIMESTAMP,
    };
    use super::super::sentry_client_secret::SentryClientSecret;
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
    };

    type HmacSha256 = Hmac<Sha256>;

    const TEST_SECRET: &str = "test-secret";

    fn wrap_publisher(
        publisher: MockJetStreamPublisher,
    ) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
        ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        )
    }

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn test_config() -> SentryConfig {
        SentryConfig {
            client_secret: SentryClientSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("sentry").unwrap(),
            stream_name: NatsToken::new("SENTRY").unwrap(),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        router(wrap_publisher(publisher), &test_config())
    }

    fn webhook_request(
        body: &[u8],
        resource: &str,
        timestamp: &str,
        request_id: &str,
        signature: Option<&str>,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_RESOURCE, resource)
            .header(HEADER_TIMESTAMP, timestamp)
            .header(HEADER_REQUEST_ID, request_id);

        if let Some(signature) = signature {
            builder = builder.header(HEADER_SIGNATURE, signature);
        }

        builder.body(Body::from(body.to_vec())).unwrap()
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "SENTRY");
        assert_eq!(streams[0].subjects, vec!["sentry.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn provision_propagates_error() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        js.fail_next();
        let config = test_config();

        let result = provision(&js, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn valid_webhook_publishes_to_nats_and_returns_200() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created","data":{}}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "sentry.issue.created");
        assert_eq!(messages[0].payload.as_ref(), body);
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .unwrap()
                .as_str(),
            "req-1"
        );
        assert_eq!(messages[0].headers.get(NATS_HEADER_RESOURCE).unwrap().as_str(), "issue");
        assert_eq!(messages[0].headers.get(NATS_HEADER_ACTION).unwrap().as_str(), "created");
        assert_eq!(
            messages[0].headers.get(NATS_HEADER_REQUEST_ID).unwrap().as_str(),
            "req-1"
        );
        assert_eq!(
            messages[0].headers.get(NATS_HEADER_TIMESTAMP).unwrap().as_str(),
            "1711315768"
        );
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created"}"#;

        let response = app
            .oneshot(webhook_request(body, "issue", "1711315768", "req-1", None))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created"}"#;

        let response = app
            .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some("not-valid")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_resource_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_SIGNATURE, signature)
            .body(Body::from(body.to_vec()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_timestamp_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_RESOURCE, "issue")
            .header(HEADER_REQUEST_ID, "req-1")
            .header(HEADER_SIGNATURE, signature)
            .body(Body::from(body.to_vec()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_request_id_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_RESOURCE, "issue")
            .header(HEADER_TIMESTAMP, "1711315768")
            .header(HEADER_SIGNATURE, signature)
            .body(Body::from(body.to_vec()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_payload_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"not-json"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn unsupported_resource_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"created"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(
                body,
                "organization",
                "1711315768",
                "req-1",
                Some(&signature),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[test]
    fn sentry_resource_as_str_covers_all_documented_values() {
        assert_eq!(SentryResource::Installation.as_str(), "installation");
        assert_eq!(SentryResource::EventAlert.as_str(), "event_alert");
        assert_eq!(SentryResource::Issue.as_str(), "issue");
        assert_eq!(SentryResource::MetricAlert.as_str(), "metric_alert");
        assert_eq!(SentryResource::Error.as_str(), "error");
        assert_eq!(SentryResource::Comment.as_str(), "comment");
        assert_eq!(SentryResource::Seer.as_str(), "seer");
    }

    #[tokio::test]
    async fn invalid_action_token_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"issue.created"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn publish_error_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher);
        let body = br#"{"action":"created"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
