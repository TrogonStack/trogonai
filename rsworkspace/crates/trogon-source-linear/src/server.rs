use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::config::LinearConfig;
use crate::config::LinearWebhookSecret;
use crate::constants::{HTTP_BODY_SIZE_MAX, NATS_HEADER_REJECT_REASON};
use crate::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap,
    http::StatusCode, routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingWebhookTimestamp,
    StaleWebhookTimestamp,
    MissingType,
    InvalidType,
    MissingAction,
    InvalidAction,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingWebhookTimestamp => "missing_webhook_timestamp",
            Self::StaleWebhookTimestamp => "stale_webhook_timestamp",
            Self::MissingType => "missing_type",
            Self::InvalidType => "invalid_type",
            Self::MissingAction => "missing_action",
            Self::InvalidAction => "invalid_action",
        }
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Linear event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("linear");
        StatusCode::INTERNAL_SERVER_ERROR
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
        StatusCode::BAD_REQUEST
    } else {
        outcome.log_on_error("linear.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: LinearWebhookSecret,
    subject_prefix: NatsToken,
    timestamp_tolerance: Option<NonZeroDuration>,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &LinearConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.to_string(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age.into(),
        ..Default::default()
    })
    .await?;

    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    let stream_name = config.stream_name.as_str();
    info!(stream = stream_name, max_age_secs, "JetStream stream ready");
    Ok(())
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &LinearConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        timestamp_tolerance: config.timestamp_tolerance,
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
    name = "linear.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        action = tracing::field::Empty,
        webhook_id = tracing::field::Empty,
        subject = tracing::field::Empty,
        messaging.message.body.size = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = headers
        .get("linear-signature")
        .and_then(|v| v.to_str().ok());

    match sig {
        Some(sig) if signature::verify(state.webhook_secret.as_str(), &body, sig) => {}
        Some(_) => {
            warn!("Invalid Linear webhook signature");
            return StatusCode::UNAUTHORIZED;
        }
        None => {
            warn!("Missing linear-signature header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse Linear webhook body as JSON");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::InvalidJson,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    if let Some(tolerance) = state.timestamp_tolerance {
        let Some(ts_ms) = parsed.get("webhookTimestamp").and_then(|v| v.as_u64()) else {
            warn!("Missing or malformed 'webhookTimestamp' field");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::MissingWebhookTimestamp,
                body,
                state.nats_ack_timeout,
            )
            .await;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age_ms = now_ms.saturating_sub(ts_ms);
        if age_ms > Duration::from(tolerance).as_millis() as u64 {
            warn!(
                age_ms,
                tolerance_ms = Duration::from(tolerance).as_millis() as u64,
                "Stale webhookTimestamp — potential replay attack"
            );
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::StaleWebhookTimestamp,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    }

    let Some(raw_type) = parsed.get("type").and_then(|v| v.as_str()) else {
        warn!("Missing 'type' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };
    let Ok(event_type) = NatsToken::new(raw_type) else {
        warn!("Invalid 'type' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let Some(raw_action) = parsed.get("action").and_then(|v| v.as_str()) else {
        warn!("Missing 'action' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingAction,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };
    let Ok(action) = NatsToken::new(raw_action) else {
        warn!("Invalid 'action' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidAction,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let webhook_id = parsed
        .get("webhookId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}.{}", state.subject_prefix, event_type, action);

    let span = tracing::Span::current();
    span.record("event_type", event_type.as_str());
    span.record("action", action.as_str());
    span.record("webhook_id", &webhook_id);
    span.record("subject", &subject);
    span.record("messaging.message.body.size", body.len());

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("Nats-Msg-Id", webhook_id.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher,
        MockObjectStore,
    };

    type HmacSha256 = Hmac<Sha256>;

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

    const TEST_SECRET: &str = "test-secret";

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("valid key length");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn test_config() -> LinearConfig {
        LinearConfig {
            webhook_secret: LinearWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("linear").expect("valid token"),
            stream_name: NatsToken::new("LINEAR").expect("valid token"),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            timestamp_tolerance: NonZeroDuration::from_secs(60).ok(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        router(wrap_publisher(publisher), &test_config())
    }

    fn webhook_request(body: &[u8], sig: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");

        if let Some(s) = sig {
            builder = builder.header("linear-signature", s);
        }

        builder
            .body(Body::from(body.to_vec()))
            .expect("valid request")
    }

    fn valid_body() -> Vec<u8> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;

        serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookId": "wh_test_123",
            "webhookTimestamp": now_ms,
            "data": {}
        }))
        .expect("valid JSON")
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
        assert_eq!(messages[0].subject, "linear.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|v| v.as_str()),
            Some(expected_reason),
        );
    }

    fn assert_no_publishes(publisher: &MockJetStreamPublisher) {
        assert!(
            publisher.published_messages().is_empty(),
            "expected no publishes"
        );
    }

    #[tokio::test]
    async fn valid_webhook_returns_200() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_body();
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "linear.Issue.create");
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "LINEAR");
        assert_eq!(streams[0].subjects, vec!["linear.>"]);
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
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_body();

        let resp = app
            .oneshot(webhook_request(&body, None))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_body();

        let resp = app
            .oneshot(webhook_request(&body, Some("bad-sig")))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"not json";
        let sig = compute_sig(TEST_SECRET, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn missing_webhook_timestamp_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create"
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_webhook_timestamp");
    }

    #[tokio::test]
    async fn stale_webhook_timestamp_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookTimestamp": 1_000_000_000_000_u64
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "stale_webhook_timestamp");
    }

    #[tokio::test]
    async fn missing_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "action": "create",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_type");
    }

    #[tokio::test]
    async fn invalid_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "has space",
            "action": "create",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_type");
    }

    #[tokio::test]
    async fn missing_action_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_action");
    }

    #[tokio::test]
    async fn invalid_action_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "has.dot",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_action");
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = valid_body();
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn dlq_publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = b"not json";
        let sig = compute_sig(TEST_SECRET, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn timestamp_tolerance_disabled_skips_check() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let mut config = test_config();
        config.timestamp_tolerance = None;
        let app = router(wrap_publisher(publisher.clone()), &config);

        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookId": "wh_no_ts",
            "webhookTimestamp": 1_000_000_000_000_u64
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "linear.Issue.create");
    }
}
