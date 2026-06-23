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
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

const TEST_NOW: u64 = 1_700_000_000;

use trogon_std::time::FixedEpochClock;

fn compute_sig(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

const TEST_SECRET: &str = "test-secret";

fn current_timestamp() -> String {
    TEST_NOW.to_string()
}

fn test_config() -> SlackConfig {
    SlackConfig {
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        transport: super::super::config::SlackTransportConfig::Webhook(SlackWebhookConfig {
            signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
        }),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    let config = test_config();
    router_with_clock(
        wrap_publisher(publisher),
        &config,
        config.webhook().unwrap(),
        FixedEpochClock::from_secs(TEST_NOW),
    )
}

fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
    assert_eq!(messages[0].subject, "slack.unroutable");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
        Some(expected_reason),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_PAYLOAD_KIND).map(|v| v.as_str()),
        Some("unroutable"),
    );
}

fn event_callback_body(event_type: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "event": {
            "type": event_type,
            "text": "hello"
        }
    }))
    .unwrap()
}

fn webhook_request(body: &[u8], timestamp: &str, sig: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_TIMESTAMP, timestamp);

    if let Some(s) = sig {
        builder = builder.header(HEADER_SIGNATURE, s);
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
    assert_eq!(streams[0].name, "SLACK");
    assert_eq!(streams[0].subjects, vec!["slack.>"]);
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
async fn router_wrapper_mounts_webhook_route() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let config = test_config();
    let app = router(wrap_publisher(publisher), &config, config.webhook().unwrap());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn event_callback_publishes_to_nats_and_returns_200() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.event.message");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_TYPE).map(|v| v.as_str()),
        Some("message"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_ID).map(|v| v.as_str()),
        Some("Ev01ABC123"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_TEAM_ID).map(|v| v.as_str()),
        Some("T01ABC"),
    );
}

#[tokio::test]
async fn url_verification_returns_challenge() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "url_verification",
        "challenge": "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let resp_body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(
        std::str::from_utf8(&resp_body).unwrap(),
        "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
    );

    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn invalid_signature_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let ts = current_timestamp();

    let resp = app
        .oneshot(webhook_request(&body, &ts, Some("v0=deadbeef")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_signature_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let ts = current_timestamp();

    let resp = app.oneshot(webhook_request(&body, &ts, None)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_timestamp_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let sig = compute_sig(TEST_SECRET, "1234567890", &body);

    let req = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_SIGNATURE, sig)
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_numeric_timestamp_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let sig = compute_sig(TEST_SECRET, "not-a-number", &body);

    let resp = app
        .oneshot(webhook_request(&body, "not-a-number", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn stale_timestamp_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let stale_ts = "1000000000";
    let sig = compute_sig(TEST_SECRET, stale_ts, &body);

    let resp = app.oneshot(webhook_request(&body, stale_ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn invalid_json_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"not json at all";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body);

    let resp = app.oneshot(webhook_request(body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_json");
}

#[tokio::test]
async fn unhandled_payload_type_returns_200() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "app_rate_limited"
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "unhandled_payload_type");
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn subject_uses_configured_prefix() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let config = SlackConfig {
        subject_prefix: NatsToken::new("custom").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        transport: super::super::config::SlackTransportConfig::Webhook(SlackWebhookConfig {
            signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
        }),
    };

    let state = AppState {
        bridge: SlackBridge::new(wrap_publisher(publisher.clone()), &config),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore, FixedEpochClock>),
        )
        .with_state(state);

    let body = event_callback_body("app_mention");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["custom.event.app_mention"]);
}

#[tokio::test]
async fn missing_event_type_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "event": {}
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "missing_event_type");
}

#[tokio::test]
async fn missing_event_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "team_id": "T01ABC",
        "event": { "type": "message" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "missing_event_id");
}

mod ack_test_support;

use ack_test_support::AckFailPublisher;
use async_nats::subject::ToSubject;

#[tokio::test]
async fn ack_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = AckFailPublisher::failing();
    let config = test_config();

    let state = AppState {
        bridge: SlackBridge::new(
            ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<AckFailPublisher, MockObjectStore, FixedEpochClock>),
        )
        .with_state(state);

    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn ack_timeout_returns_500() {
    let _guard = tracing_guard();
    let publisher = AckFailPublisher::hanging();
    let config = SlackConfig {
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_millis(10).unwrap(),
        transport: super::super::config::SlackTransportConfig::Webhook(SlackWebhookConfig {
            signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
        }),
    };

    let state = AppState {
        bridge: SlackBridge::new(
            ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<AckFailPublisher, MockObjectStore, FixedEpochClock>),
        )
        .with_state(state);

    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

fn form_request(body: &[u8], timestamp: &str, sig: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_TIMESTAMP, timestamp)
        .header(HEADER_SIGNATURE, sig)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

#[tokio::test]
async fn interaction_block_actions_publishes_to_nats() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let interaction_payload = serde_json::json!({
        "type": "block_actions",
        "trigger_id": "trigger123",
        "team": { "id": "T01ABC" },
        "user": { "id": "U01ABC" },
        "actions": [{ "action_id": "btn_click", "type": "button" }]
    });
    let form_body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes()).collect::<String>()
    );
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.interaction.block_actions");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_PAYLOAD_KIND).map(|v| v.as_str()),
        Some("interaction"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_TEAM_ID).map(|v| v.as_str()),
        Some("T01ABC"),
    );
}

#[tokio::test]
async fn interaction_view_submission_publishes_to_nats() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let interaction_payload = serde_json::json!({
        "type": "view_submission",
        "trigger_id": "trigger456",
        "team": { "id": "T01ABC" },
        "user": { "id": "U01ABC" },
        "view": { "callback_id": "my_modal" }
    });
    let form_body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes()).collect::<String>()
    );
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        publisher.published_subjects(),
        vec!["slack.interaction.view_submission"]
    );
}

#[tokio::test]
async fn interaction_missing_trigger_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let interaction_payload = serde_json::json!({
        "type": "view_closed",
        "team": { "id": "T01ABC" },
        "user": { "id": "U01ABC" }
    });
    let form_body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes()).collect::<String>()
    );
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "missing_interaction_trigger_id");
}

#[tokio::test]
async fn slash_command_publishes_to_nats() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let form_body =
        "command=%2Ftrogon&text=hello+world&team_id=T01ABC&trigger_id=cmd789&user_id=U01ABC&channel_id=C01ABC";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.command.trogon");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_PAYLOAD_KIND).map(|v| v.as_str()),
        Some("command"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_TYPE).map(|v| v.as_str()),
        Some("/trogon"),
    );
}

#[tokio::test]
async fn slash_command_missing_trigger_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let form_body = "command=%2Ftrogon&text=hello&team_id=T01ABC&user_id=U01ABC&channel_id=C01ABC";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "missing_command_trigger_id");
}

#[tokio::test]
async fn form_payload_with_invalid_json_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let form_body = "payload=not-valid-json";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_interaction_json");
}

#[tokio::test]
async fn invalid_utf8_form_payload_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let body: &[u8] = &[0xff, 0xfe, 0xfd];
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body);

    let resp = app.oneshot(form_request(body, &ts, &sig)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_utf8_form");
}

#[tokio::test]
async fn unrecognized_form_payload_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let form_body = "foo=bar&baz=qux";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "unrecognized_form");
}

#[tokio::test]
async fn interaction_missing_type_publishes_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let interaction_payload = serde_json::json!({
        "trigger_id": "trigger123",
        "team": { "id": "T01ABC" }
    });
    let form_body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes()).collect::<String>()
    );
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "missing_interaction_type");
}
