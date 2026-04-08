use std::fmt;
use std::time::Duration;

use crate::config::GitLabWebhookSecret;
use crate::config::GitlabConfig;
use crate::constants::{
    HEADER_EVENT, HEADER_EVENT_UUID, HEADER_IDEMPOTENCY_KEY, HEADER_INSTANCE, HEADER_TOKEN,
    HEADER_WEBHOOK_UUID, HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT, NATS_HEADER_EVENT_UUID,
    NATS_HEADER_INSTANCE, NATS_HEADER_REJECT_REASON, NATS_HEADER_WEBHOOK_UUID,
};
use crate::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap,
    http::StatusCode, routing::post,
};
use std::future::Future;
use std::pin::Pin;
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    MissingEventHeader,
    InvalidEventToken,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingEventHeader => "missing_event_header",
            Self::InvalidEventToken => "invalid_event_token",
        }
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;
    if outcome.is_ok() {
        // Return 200 so GitLab doesn't count this as a failure and
        // auto-disable the webhook after repeated 4xx responses.
        StatusCode::OK
    } else {
        outcome.log_on_error("gitlab.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published GitLab event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("gitlab");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: GitLabWebhookSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &GitlabConfig) -> Result<(), C::Error> {
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
    config: &GitlabConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = StatusCode> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

#[instrument(
    name = "gitlab.webhook",
    skip_all,
    fields(
        event = tracing::field::Empty,
        webhook_uuid = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let token = headers.get(HEADER_TOKEN).and_then(|v| v.to_str().ok());

    match token {
        Some(token) => {
            if let Err(e) = signature::verify(state.webhook_secret.as_str(), token) {
                warn!(reason = %e, "GitLab webhook token validation failed");
                return StatusCode::UNAUTHORIZED;
            }
        }
        None => {
            warn!("Missing X-Gitlab-Token header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let Some(raw_event) = headers.get(HEADER_EVENT).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-GitLab-Event header");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingEventHeader,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let event_token: String = raw_event
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();

    let Ok(event_token) = NatsToken::new(&event_token) else {
        warn!(raw_event, "X-GitLab-Event normalized to invalid NATS token");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidEventToken,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let webhook_uuid = headers
        .get(HEADER_WEBHOOK_UUID)
        .and_then(|v| v.to_str().ok());
    let event_uuid = headers.get(HEADER_EVENT_UUID).and_then(|v| v.to_str().ok());
    let idempotency_key = headers
        .get(HEADER_IDEMPOTENCY_KEY)
        .and_then(|v| v.to_str().ok());
    let instance = headers.get(HEADER_INSTANCE).and_then(|v| v.to_str().ok());

    let subject = format!("{}.{}", state.subject_prefix, event_token);

    let span = tracing::Span::current();
    span.record("event", raw_event);
    span.record("webhook_uuid", webhook_uuid.unwrap_or("unknown"));
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_EVENT, raw_event);
    if let Some(id) = idempotency_key {
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, id);
    }
    if let Some(uuid) = webhook_uuid {
        nats_headers.insert(NATS_HEADER_WEBHOOK_UUID, uuid);
    }
    if let Some(euuid) = event_uuid {
        nats_headers.insert(NATS_HEADER_EVENT_UUID, euuid);
    }
    if let Some(inst) = instance {
        nats_headers.insert(NATS_HEADER_INSTANCE, inst);
    }

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher,
        MockObjectStore,
    };

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

    fn test_config() -> GitlabConfig {
        GitlabConfig {
            webhook_secret: GitLabWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("gitlab").unwrap(),
            stream_name: NatsToken::new("GITLAB").unwrap(),
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

    fn webhook_request(body: &[u8], event: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_EVENT, event)
            .header(HEADER_WEBHOOK_UUID, "wh-uuid-test")
            .header(HEADER_IDEMPOTENCY_KEY, "idem-key-test")
            .header(HEADER_INSTANCE, "https://gitlab.example.com")
            .header(HEADER_EVENT_UUID, "evt-uuid-test");

        if let Some(t) = token {
            builder = builder.header(HEADER_TOKEN, t);
        }

        builder.body(Body::from(body.to_vec())).unwrap()
    }

    #[test]
    fn reject_reason_as_str() {
        assert_eq!(
            RejectReason::MissingEventHeader.as_str(),
            "missing_event_header"
        );
        assert_eq!(
            RejectReason::InvalidEventToken.as_str(),
            "invalid_event_token"
        );
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "GITLAB");
        assert_eq!(streams[0].subjects, vec!["gitlab.>"]);
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
        let body = br#"{"ref":"refs/heads/main"}"#;
        let resp = app
            .oneshot(webhook_request(body, "push", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "gitlab.push");
        assert_eq!(messages[0].payload, Bytes::from(&body[..]));
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT)
                .map(|v| v.as_str()),
            Some("push"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_WEBHOOK_UUID)
                .map(|v| v.as_str()),
            Some("wh-uuid-test"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_INSTANCE)
                .map(|v| v.as_str()),
            Some("https://gitlab.example.com"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_UUID)
                .map(|v| v.as_str()),
            Some("evt-uuid-test"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|v| v.as_str()),
            Some("idem-key-test"),
        );
    }

    #[tokio::test]
    async fn valid_signature_publishes_and_returns_200() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"action":"opened"}"#;
        let resp = app
            .oneshot(webhook_request(body, "pull_request", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["gitlab.pull_request"]);
    }

    #[tokio::test]
    async fn invalid_signature_returns_401_and_does_not_publish() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let resp = app
            .oneshot(webhook_request(b"{}", "push", Some("wrong-token")))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let resp = app
            .oneshot(webhook_request(b"{}", "push", None))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn missing_event_header_publishes_to_dlq() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"{}";
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_TOKEN, TEST_SECRET)
            .body(Body::from(&body[..]))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "gitlab.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|v| v.as_str()),
            Some("missing_event_header"),
        );
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = b"{}";
        let resp = app
            .oneshot(webhook_request(body, "push", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn subject_uses_configured_prefix() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();

        let state = AppState {
            publisher: wrap_publisher(publisher.clone()),
            webhook_secret: GitLabWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("custom").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = b"{}";
        let resp = app
            .oneshot(webhook_request(body, "issues", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["custom.issues"]);
    }

    #[tokio::test]
    async fn empty_body_publishes_successfully() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"";
        let resp = app
            .oneshot(webhook_request(body, "ping", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_payloads(), vec![Bytes::new()]);
    }

    #[tokio::test]
    async fn missing_idempotency_key_skips_dedup_id() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();

        let state = AppState {
            publisher: wrap_publisher(publisher.clone()),
            webhook_secret: GitLabWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("gitlab").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
            )
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_EVENT, "push")
            .header(HEADER_TOKEN, TEST_SECRET)
            .body(Body::from(&b"{}"[..]))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .is_none(),
            "should not set Nats-Msg-Id when Idempotency-Key is absent"
        );
    }

    #[tokio::test]
    async fn event_with_spaces_normalizes_to_underscores() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"{}";
        let resp = app
            .oneshot(webhook_request(
                body,
                "Merge Request Hook",
                Some(TEST_SECRET),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["gitlab.merge_request_hook"]
        );
    }

    #[tokio::test]
    async fn dlq_publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = b"{}";
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_TOKEN, TEST_SECRET)
            .body(Body::from(&body[..]))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    mod ack_test_support {
        use super::*;
        use async_nats::jetstream::publish::PublishAck;
        use std::sync::Arc;
        use std::sync::Mutex;
        use trogon_nats::mocks::MockError;

        #[derive(Clone)]
        enum AckBehavior {
            Fail,
            Hang,
        }

        #[derive(Clone)]
        pub struct AckFailPublisher {
            behavior: Arc<Mutex<AckBehavior>>,
        }

        impl AckFailPublisher {
            pub fn failing() -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Fail)),
                }
            }

            pub fn hanging() -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Hang)),
                }
            }
        }

        pub enum AckFuture {
            Fail,
            Hang,
        }

        impl IntoFuture for AckFuture {
            type Output = Result<PublishAck, MockError>;
            type IntoFuture = std::pin::Pin<Box<dyn Future<Output = Self::Output> + Send>>;

            fn into_future(self) -> Self::IntoFuture {
                match self {
                    AckFuture::Fail => {
                        Box::pin(async { Err(MockError("simulated ack failure".to_string())) })
                    }
                    AckFuture::Hang => Box::pin(std::future::pending()),
                }
            }
        }

        impl JetStreamPublisher for AckFailPublisher {
            type PublishError = MockError;
            type AckFuture = AckFuture;

            async fn publish_with_headers<S: ToSubject + Send>(
                &self,
                _subject: S,
                _headers: async_nats::HeaderMap,
                _payload: Bytes,
            ) -> Result<AckFuture, MockError> {
                let behavior = self.behavior.lock().unwrap().clone();
                match behavior {
                    AckBehavior::Fail => Ok(AckFuture::Fail),
                    AckBehavior::Hang => Ok(AckFuture::Hang),
                }
            }
        }
    }

    use ack_test_support::AckFailPublisher;
    use async_nats::subject::ToSubject;

    #[tokio::test]
    async fn ack_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::failing();

        let state = AppState {
            publisher: ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            webhook_secret: GitLabWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("gitlab").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<AckFailPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = b"{}";
        let resp = app
            .oneshot(webhook_request(body, "push", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn ack_timeout_returns_500() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::hanging();

        let state = AppState {
            publisher: ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            webhook_secret: GitLabWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("gitlab").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_millis(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<AckFailPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = b"{}";
        let resp = app
            .oneshot(webhook_request(body, "push", Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
