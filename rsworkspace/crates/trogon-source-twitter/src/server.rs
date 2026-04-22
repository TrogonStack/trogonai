use std::fmt;
use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument, warn};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_std::NonZeroDuration;

use crate::config::{TwitterConfig, TwitterConsumerSecret};
use crate::constants::{
    HEADER_SIGNATURE, HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT_TYPE, NATS_HEADER_PAYLOAD_KIND,
    NATS_HEADER_REJECT_REASON,
};
use crate::signature;

const ACCOUNT_ACTIVITY_EVENT_TYPES: &[&str] = &[
    "tweet_create_events",
    "favorite_events",
    "follow_events",
    "unfollow_events",
    "block_events",
    "unblock_events",
    "mute_events",
    "unmute_events",
    "user_event",
    "direct_message_events",
    "direct_message_indicate_typing_events",
    "direct_message_mark_read_events",
    "tweet_delete_events",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingEventType,
    InvalidEventType,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingEventType => "missing_event_type",
            Self::InvalidEventType => "invalid_event_type",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadKind {
    XActivity,
    AccountActivity,
    FilteredStream,
}

impl PayloadKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::XActivity => "x_activity",
            Self::AccountActivity => "account_activity",
            Self::FilteredStream => "filtered_stream",
        }
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    consumer_secret: TwitterConsumerSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

#[derive(Deserialize)]
struct CrcQuery {
    crc_token: String,
}

#[derive(Serialize)]
struct CrcResponse {
    response_token: String,
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Twitter/X event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("twitter");
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
    headers.insert(NATS_HEADER_PAYLOAD_KIND, "unroutable");

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;

    if outcome.is_ok() {
        StatusCode::OK
    } else {
        outcome.log_on_error("twitter.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

pub async fn provision<C: JetStreamContext>(
    js: &C,
    config: &TwitterConfig,
) -> Result<(), C::Error> {
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
    config: &TwitterConfig,
) -> Router {
    let state = AppState {
        publisher,
        consumer_secret: config.consumer_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route(
            "/webhook",
            get(handle_crc::<P, S>).post(handle_webhook::<P, S>),
        )
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[instrument(name = "twitter.crc", skip_all)]
async fn handle_crc<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    Query(query): Query<CrcQuery>,
) -> Result<Json<CrcResponse>, StatusCode> {
    if query.crc_token.is_empty() {
        warn!("Empty crc_token query parameter");
        return Err(StatusCode::BAD_REQUEST);
    }

    Ok(Json(CrcResponse {
        response_token: signature::crc_response_token(
            state.consumer_secret.as_str(),
            &query.crc_token,
        ),
    }))
}

#[instrument(
    name = "twitter.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        payload_kind = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(signature_header) = headers
        .get(HEADER_SIGNATURE)
        .and_then(|value| value.to_str().ok())
    else {
        warn!("Missing x-twitter-webhooks-signature header");
        return StatusCode::UNAUTHORIZED;
    };

    if let Err(error) = signature::verify(state.consumer_secret.as_str(), &body, signature_header) {
        warn!(reason = %error, "Twitter/X webhook signature validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let payload = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(error = %error, "Failed to parse Twitter/X webhook payload");
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

    let (event_type, payload_kind) = match resolve_event_type(&payload) {
        Ok(event_type) => event_type,
        Err(reason) => {
            warn!(
                reason = reason.as_str(),
                "Unable to classify Twitter/X webhook payload"
            );
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                reason,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    let subject = format!("{}.{}", state.subject_prefix, event_type);
    let span = tracing::Span::current();
    span.record("event_type", event_type.as_str());
    span.record("payload_kind", payload_kind.as_str());
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
    nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, payload_kind.as_str());

    if let Some(message_id) = extract_message_id(&payload, event_type.as_str()) {
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, message_id.as_str());
    }

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

fn resolve_event_type(
    payload: &serde_json::Value,
) -> Result<(DottedNatsToken, PayloadKind), RejectReason> {
    if let Some(event_type) = payload
        .get("data")
        .and_then(|value| value.get("event_type"))
        .and_then(serde_json::Value::as_str)
    {
        return DottedNatsToken::new(event_type)
            .map(|event_type| (event_type, PayloadKind::XActivity))
            .map_err(|_| RejectReason::InvalidEventType);
    }

    if payload.get("matching_rules").is_some() {
        return Ok((
            DottedNatsToken::new("filtered_stream").expect("literal token is valid"),
            PayloadKind::FilteredStream,
        ));
    }

    let Some(object) = payload.as_object() else {
        return Err(RejectReason::MissingEventType);
    };

    let account_activity_types = ACCOUNT_ACTIVITY_EVENT_TYPES
        .iter()
        .copied()
        .filter(|event_type| object.contains_key(*event_type))
        .collect::<Vec<_>>();

    if account_activity_types.is_empty() {
        return Err(RejectReason::MissingEventType);
    }

    let event_type = if account_activity_types.len() == 1 {
        account_activity_types[0]
    } else {
        "account_activity"
    };

    DottedNatsToken::new(event_type)
        .map(|event_type| (event_type, PayloadKind::AccountActivity))
        .map_err(|_| RejectReason::InvalidEventType)
}

fn extract_message_id(payload: &serde_json::Value, event_type: &str) -> Option<String> {
    payload
        .get("data")
        .and_then(value_id)
        .or_else(|| {
            payload
                .get("data")
                .and_then(|value| value.get("payload"))
                .and_then(value_id)
        })
        .or_else(|| {
            if event_type == PayloadKind::AccountActivity.as_str() {
                return None;
            }

            payload
                .get(event_type)
                .and_then(|event_value| match event_value {
                    serde_json::Value::Array(items) if items.len() == 1 => value_id(&items[0]),
                    serde_json::Value::Object(_) => value_id(event_value),
                    _ => None,
                })
        })
        .map(|message_id| format!("{event_type}:{message_id}"))
}

fn value_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("id_str")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            value.get("id").and_then(|id| match id {
                serde_json::Value::String(value) => Some(value.clone()),
                serde_json::Value::Number(value) => Some(value.to_string()),
                _ => None,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::jetstream::publish::PublishAck;
    use async_nats::subject::ToSubject;
    use axum::{body::Body, http::Request};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::future::{Future, IntoFuture};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, JetStreamPublisher, MaxPayload, MockJetStreamContext,
        MockJetStreamPublisher, MockObjectStore,
    };
    use trogon_nats::mocks::MockError;

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

    fn test_config() -> TwitterConfig {
        TwitterConfig {
            consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("twitter").unwrap(),
            stream_name: NatsToken::new("TWITTER").unwrap(),
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

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn webhook_request(body: &[u8], signature: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");
        if let Some(signature) = signature {
            builder = builder.header(HEADER_SIGNATURE, signature);
        }
        builder.body(Body::from(body.to_vec())).unwrap()
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some(expected_reason),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("unroutable"),
        );
    }

    mod ack_test_support {
        use super::*;

        #[derive(Clone)]
        enum AckBehavior {
            Fail,
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
        }

        pub enum AckFuture {
            Fail,
        }

        impl IntoFuture for AckFuture {
            type Output = Result<PublishAck, MockError>;
            type IntoFuture = std::pin::Pin<Box<dyn Future<Output = Self::Output> + Send>>;

            fn into_future(self) -> Self::IntoFuture {
                match self {
                    AckFuture::Fail => {
                        Box::pin(async { Err(MockError("simulated ack failure".to_string())) })
                    }
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
                }
            }
        }
    }

    use ack_test_support::AckFailPublisher;

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "TWITTER");
        assert_eq!(streams[0].subjects, vec!["twitter.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn crc_request_returns_response_token() {
        let _guard = tracing_guard();
        let app = mock_app(MockJetStreamPublisher::new());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/webhook?crc_token=challenge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload["response_token"],
            signature::crc_response_token(TEST_SECRET, "challenge"),
        );
    }

    #[tokio::test]
    async fn crc_request_without_token_returns_bad_request() {
        let _guard = tracing_guard();
        let app = mock_app(MockJetStreamPublisher::new());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/webhook")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn x_activity_event_publishes_to_event_type_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "data": {
                "event_type": "profile.update.bio",
                "payload": {
                    "before": "Mars & Cars",
                    "after": "Mars, Cars & AI"
                }
            }
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.profile.update.bio");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_TYPE)
                .map(|value| value.as_str()),
            Some("profile.update.bio"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("x_activity"),
        );
    }

    #[tokio::test]
    async fn account_activity_event_publishes_to_event_bucket_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "for_user_id": "2244994945",
            "favorite_events": [{
                "id": "fav-1",
                "created_at": "Mon Mar 26 16:33:26 +0000 2018"
            }]
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.favorite_events");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("account_activity"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("favorite_events:fav-1"),
        );
    }

    #[tokio::test]
    async fn generic_account_activity_fallback_omits_message_id() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "for_user_id": "2244994945",
            "favorite_events": [{
                "id": "fav-1"
            }],
            "follow_events": [{
                "id": "follow-1"
            }]
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.account_activity");
        assert_eq!(
            messages[0].headers.get(async_nats::header::NATS_MESSAGE_ID),
            None
        );
    }

    #[tokio::test]
    async fn filtered_stream_payload_publishes_to_filtered_stream_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "data": {
                "id": "1234567890",
                "text": "hello world"
            },
            "matching_rules": [
                { "id": "rule-1", "tag": "cats" }
            ]
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.filtered_stream");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("filtered_stream"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("filtered_stream:1234567890"),
        );
    }

    #[tokio::test]
    async fn invalid_signature_returns_unauthorized() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let response = app
            .oneshot(webhook_request(
                br#"{"data":{"event_type":"profile.update.bio"}}"#,
                Some("sha256=deadbeef"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"{not-json";
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn unsupported_shape_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"hello":"world"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "missing_event_type");
    }

    #[tokio::test]
    async fn invalid_event_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "data": {
                "event_type": "invalid token"
            }
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_event_type");
    }

    #[tokio::test]
    async fn publish_ack_failure_returns_internal_server_error() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::failing();

        let state = AppState {
            publisher: ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("twitter").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                get(handle_crc::<AckFailPublisher, MockObjectStore>)
                    .post(handle_webhook::<AckFailPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = br#"{"data":{"event_type":"profile.update.bio"}}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn unroutable_publish_ack_failure_returns_internal_server_error() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::failing();

        let state = AppState {
            publisher: ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("twitter").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                get(handle_crc::<AckFailPublisher, MockObjectStore>)
                    .post(handle_webhook::<AckFailPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = b"{not-json";
        let signature = compute_sig(TEST_SECRET, body);

        let response = app
            .oneshot(webhook_request(body, Some(&signature)))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn resolve_event_type_rejects_non_object_payloads() {
        let payload = serde_json::json!(42);

        assert_eq!(
            resolve_event_type(&payload),
            Err(RejectReason::MissingEventType)
        );
    }

    #[test]
    fn resolve_event_type_falls_back_for_multiple_account_activity_keys() {
        let payload = serde_json::json!({
            "favorite_events": [{"id": "fav-1"}],
            "follow_events": [{"id": "follow-1"}]
        });

        let (event_type, payload_kind) = resolve_event_type(&payload).expect("event type");
        assert_eq!(event_type.as_str(), "account_activity");
        assert_eq!(payload_kind, PayloadKind::AccountActivity);
    }

    #[test]
    fn extract_message_id_uses_object_payload_ids() {
        let payload = serde_json::json!({
            "user_event": {
                "id": "user-event-1"
            }
        });

        assert_eq!(
            extract_message_id(&payload, "user_event").as_deref(),
            Some("user_event:user-event-1")
        );
    }

    #[test]
    fn extract_message_id_skips_multi_item_account_activity_arrays() {
        let payload = serde_json::json!({
            "favorite_events": [
                {"id": "fav-1"},
                {"id": "fav-2"}
            ]
        });

        assert_eq!(extract_message_id(&payload, "favorite_events"), None);
    }

    #[test]
    fn extract_message_id_omits_generic_account_activity_bucket() {
        let payload = serde_json::json!({
            "favorite_events": [{"id": "fav-1"}],
            "follow_events": [{"id": "follow-1"}]
        });

        assert_eq!(extract_message_id(&payload, "account_activity"), None);
    }

    #[test]
    fn extract_message_id_prefixes_data_ids_with_event_type() {
        let payload = serde_json::json!({
            "data": {
                "id": "1234567890"
            }
        });

        assert_eq!(
            extract_message_id(&payload, "filtered_stream").as_deref(),
            Some("filtered_stream:1234567890")
        );
    }

    #[test]
    fn value_id_supports_numeric_ids() {
        let value = serde_json::json!({
            "id": 12345
        });

        assert_eq!(value_id(&value).as_deref(), Some("12345"));
    }

    #[test]
    fn value_id_rejects_non_string_and_non_numeric_ids() {
        let value = serde_json::json!({
            "id": true
        });

        assert_eq!(value_id(&value), None);
    }
}
