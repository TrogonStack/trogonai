use super::super::constants::{
    HEADER_RETRY_NUM, HEADER_RETRY_REASON, NATS_HEADER_API_APP_ID, NATS_HEADER_ENTERPRISE_ID,
    NATS_HEADER_IS_ENTERPRISE_INSTALL, NATS_HEADER_REQUEST_TIMESTAMP, NATS_HEADER_RETRY_NUM, NATS_HEADER_RETRY_REASON,
};
use super::*;
use axum::body::Body;
use axum::http::Request;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
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
        ack_budget: AckBudget::default(),
        transport: super::super::config::SlackTransportConfig::Webhook(SlackWebhookConfig {
            signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
            max_body_size: super::super::constants::DEFAULT_HTTP_BODY_SIZE_MAX,
        }),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn acknowledging_requester() -> AdvancedMockNatsClient {
    let requester = AdvancedMockNatsClient::new();
    for subject in [
        "slack.respond.interaction.view_submission",
        "slack.respond.command.trogon",
        "slack.respond.command.deploy",
    ] {
        requester.set_response(subject, Bytes::new());
    }
    requester
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    mock_app_with_requester(publisher, acknowledging_requester())
}

fn mock_app_with_requester(publisher: MockJetStreamPublisher, requester: AdvancedMockNatsClient) -> Router {
    let config = test_config();
    router_with_clock(
        wrap_publisher(publisher),
        requester,
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
    assert_eq!(
        streams[0].subjects,
        SlackSubject::stream_filters(&test_config().subject_prefix)
    );
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    assert_eq!(streams[0].duplicate_window, Duration::from_secs(600));
}

#[tokio::test]
async fn provision_caps_the_dedup_window_at_a_shorter_max_age() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    let mut config = test_config();
    config.stream_max_age = StreamMaxAge::from_secs(120).unwrap();

    provision(&js, &config).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams[0].max_age, Duration::from_secs(120));
    assert_eq!(streams[0].duplicate_window, Duration::from_secs(120));
}

#[tokio::test]
async fn provision_widens_the_dedup_window_on_an_already_provisioned_stream() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: "SLACK".to_owned(),
        duplicate_window: Duration::from_secs(120),
        num_replicas: 3,
        max_bytes: 1_024,
        ..Default::default()
    })
    .await
    .unwrap();

    provision(&js, &test_config()).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1, "reconciled in place rather than duplicated");
    assert_eq!(streams[0].duplicate_window, Duration::from_secs(600));
    assert_eq!(
        streams[0].subjects,
        SlackSubject::stream_filters(&test_config().subject_prefix)
    );
    assert_eq!(streams[0].num_replicas, 3, "operator-set replica count survives");
    assert_eq!(streams[0].max_bytes, 1_024, "operator-set limit survives");
}

#[tokio::test]
async fn provision_narrows_a_stream_created_on_the_broad_filter() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: "SLACK".to_owned(),
        subjects: vec!["slack.>".to_owned()],
        ..Default::default()
    })
    .await
    .unwrap();

    provision(&js, &test_config()).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert!(
        streams[0].subjects.iter().all(|subject| subject != "slack.>"),
        "the broad filter would capture respond subjects"
    );
    assert!(
        !streams[0]
            .subjects
            .iter()
            .any(|subject| subject.starts_with("slack.respond"))
    );
    assert_eq!(streams[0].subjects.len(), 5);
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
    let app = router(
        wrap_publisher(publisher),
        AdvancedMockNatsClient::new(),
        &config,
        config.webhook().unwrap(),
    );

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
        "type": "future_outer_type"
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
        ack_budget: AckBudget::default(),
        transport: super::super::config::SlackTransportConfig::Webhook(SlackWebhookConfig {
            signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
            max_body_size: super::super::constants::DEFAULT_HTTP_BODY_SIZE_MAX,
        }),
    };

    let state = AppState {
        bridge: SlackBridge::new(
            wrap_publisher(publisher.clone()),
            AdvancedMockNatsClient::new(),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore, AdvancedMockNatsClient, FixedEpochClock>),
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
                ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            AdvancedMockNatsClient::new(),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<AckFailPublisher, MockObjectStore, AdvancedMockNatsClient, FixedEpochClock>),
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
        ack_budget: AckBudget::new(Some(NonZeroDuration::from_millis(10).unwrap())),
        transport: super::super::config::SlackTransportConfig::Webhook(SlackWebhookConfig {
            signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
            max_body_size: super::super::constants::DEFAULT_HTTP_BODY_SIZE_MAX,
        }),
    };

    let state = AppState {
        bridge: SlackBridge::new(
            ClaimCheckPublisher::new(
                publisher,
                ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            AdvancedMockNatsClient::new(),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<AckFailPublisher, MockObjectStore, AdvancedMockNatsClient, FixedEpochClock>),
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
async fn a_form_content_type_with_other_case_and_a_charset_is_still_a_form() {
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
    let mut request = form_request(form_body.as_bytes(), &ts, &sig);
    request.headers_mut().insert(
        "content-type",
        "Application/X-WWW-Form-Urlencoded; charset=UTF-8".parse().unwrap(),
    );

    let resp = app.oneshot(request).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.interaction.block_actions");
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
    assert_eq!(header(&messages[0].headers, "Content-Type"), Some("application/json"));
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
async fn interaction_without_trigger_id_is_published_and_keyed_by_its_view() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let interaction_payload = serde_json::json!({
        "type": "view_closed",
        "team": { "id": "T01ABC" },
        "user": { "id": "U01ABC" },
        "view": { "id": "V01" }
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
    assert_eq!(messages[0].subject, "slack.interaction.view_closed");
    assert_eq!(header(&messages[0].headers, "Nats-Msg-Id"), Some("view_closed-V01"));
}

#[tokio::test]
async fn interaction_without_any_identifier_is_keyed_by_its_bytes() {
    let _guard = tracing_guard();
    let interaction_payload = serde_json::json!({
        "type": "shortcut",
        "team": { "id": "T01ABC" },
        "callback_id": "tro"
    });
    let form_body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes()).collect::<String>()
    );
    let mut ids = Vec::new();
    for _ in 0..2 {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages[0].subject, "slack.interaction.shortcut");
        ids.push(header(&messages[0].headers, "Nats-Msg-Id").unwrap().to_owned());
    }
    assert_eq!(ids[0], ids[1]);
    assert!(ids[0].starts_with("interaction-"));
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
        header(&messages[0].headers, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
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

fn header<'a>(headers: &'a async_nats::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).map(|v| v.as_str())
}

#[tokio::test]
async fn failed_unroutable_publish_turns_a_400_into_a_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = b"not json at all";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body);

    let resp = app.oneshot(webhook_request(body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn failed_unroutable_publish_turns_a_200_into_a_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({ "type": "future_outer_type" })).unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn unroutable_message_carries_the_same_envelope_headers_as_routable_ones() {
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

    app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    let messages = publisher.published_messages();
    assert_eq!(header(&messages[0].headers, NATS_HEADER_TEAM_ID), Some("T01ABC"));
    assert_eq!(header(&messages[0].headers, NATS_HEADER_TRANSPORT), Some("webhook"));
    assert!(
        header(&messages[0].headers, "Nats-Msg-Id")
            .unwrap()
            .starts_with("unroutable-")
    );
    assert_eq!(messages[0].payload.as_ref(), body.as_slice());
}

#[tokio::test]
async fn replayed_unroutable_body_gets_the_same_message_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let ts = current_timestamp();
    let body = b"not json at all";
    let sig = compute_sig(TEST_SECRET, &ts, body);

    for _ in 0..2 {
        mock_app(publisher.clone())
            .oneshot(webhook_request(body, &ts, Some(&sig)))
            .await
            .unwrap();
    }

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        header(&messages[1].headers, "Nats-Msg-Id")
    );
    assert_eq!(header(&messages[0].headers, NATS_HEADER_TEAM_ID), Some("unknown"));
}

#[tokio::test]
async fn unroutable_message_falls_back_to_unknown_when_the_team_id_is_not_header_safe() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01\r\nX-Injected: 1",
        "event": { "type": "message" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    let messages = publisher.published_messages();
    assert_eq!(header(&messages[0].headers, NATS_HEADER_TEAM_ID), Some("unknown"));
}

#[tokio::test]
async fn routable_event_is_tagged_with_its_transport() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    let messages = publisher.published_messages();
    assert_eq!(header(&messages[0].headers, NATS_HEADER_TRANSPORT), Some("webhook"));
}

#[tokio::test]
async fn event_with_line_break_in_team_id_returns_400_and_is_kept_as_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01\r\nX-Injected: 1",
        "event": { "type": "message" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn event_with_line_break_in_event_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01\nABC",
        "team_id": "T01ABC",
        "event": { "type": "message" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn interaction_with_line_break_in_trigger_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let interaction_payload = serde_json::json!({
        "type": "block_actions",
        "trigger_id": "trigger\r\n123",
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
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn slash_command_with_line_break_in_team_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let form_body = "command=%2Ftrogon&team_id=T01%0D%0AX-Injected%3A+1&trigger_id=cmd789";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn event_type_with_subject_wildcard_is_kept_as_unroutable() {
    let _guard = tracing_guard();
    for bad in ["a.b", "a*", ">", "a b", "caf\u{e9}"] {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body(bad);
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{bad:?}");
        assert_unroutable(&publisher, "invalid_subject_token");
        assert_eq!(publisher.published_messages()[0].payload.as_ref(), body.as_slice());
    }
}

#[tokio::test]
async fn interaction_type_with_subject_separator_is_kept_as_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let interaction_payload = serde_json::json!({
        "type": "block.actions",
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
    assert_unroutable(&publisher, "invalid_subject_token");
}

#[tokio::test]
async fn command_name_with_subject_separator_is_kept_as_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let form_body = "command=%2Fa.b&team_id=T01ABC&trigger_id=cmd789";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_subject_token");
}

#[tokio::test]
async fn empty_command_is_rejected_as_missing_command() {
    let _guard = tracing_guard();
    for form_body in [
        "command=&team_id=T01ABC&trigger_id=cmd789",
        "command=%2F&team_id=T01ABC&trigger_id=cmd789",
    ] {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{form_body}");
        assert_unroutable(&publisher, "missing_command");
    }
}

fn app_with_body_limit(publisher: MockJetStreamPublisher, limit: usize) -> Router {
    let config = test_config();
    let mut webhook = config.webhook().unwrap().clone();
    webhook.max_body_size = trogon_std::HttpBodySizeMax::new(trogon_std::ByteSize::b(limit as u64)).unwrap();
    router_with_clock(
        wrap_publisher(publisher),
        AdvancedMockNatsClient::new(),
        &config,
        &webhook,
        FixedEpochClock::from_secs(TEST_NOW),
    )
}

#[tokio::test]
async fn configured_body_limit_rejects_larger_requests_and_admits_smaller_ones() {
    let _guard = tracing_guard();
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let publisher = MockJetStreamPublisher::new();
    let too_small = app_with_body_limit(publisher.clone(), body.len() - 1);
    let resp = too_small
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(publisher.published_messages().is_empty());

    let publisher = MockJetStreamPublisher::new();
    let exact = app_with_body_limit(publisher.clone(), body.len());
    let resp = exact.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_messages().len(), 1);
}

#[tokio::test]
async fn default_body_limit_admits_payloads_above_the_old_one_mebibyte_cap() {
    let _guard = tracing_guard();
    let filler = "x".repeat(2 * 1024 * 1024);
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "event": { "type": "message", "text": filler }
    }))
    .unwrap();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(start_paused = true)]
async fn unacknowledged_publish_answers_slack_inside_its_three_second_deadline() {
    let _guard = tracing_guard();
    let mut config = test_config();
    config.ack_budget = AckBudget::default();

    let state = AppState {
        bridge: SlackBridge::new(
            ClaimCheckPublisher::new(
                AckFailPublisher::hanging(),
                ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            AdvancedMockNatsClient::new(),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };
    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<AckFailPublisher, MockObjectStore, AdvancedMockNatsClient, FixedEpochClock>),
        )
        .with_state(state);
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let started = tokio::time::Instant::now();
    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(elapsed, std::time::Duration::from_millis(2800));
    assert!(elapsed < super::super::constants::SLACK_ACK_DEADLINE);
}

#[tokio::test(start_paused = true)]
async fn a_publish_that_never_returns_answers_slack_inside_its_three_second_deadline() {
    let _guard = tracing_guard();
    let mut config = test_config();
    config.ack_budget = AckBudget::default();

    let state = AppState {
        bridge: SlackBridge::new(
            ClaimCheckPublisher::new(
                AckFailPublisher::hanging_publish(),
                ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            AdvancedMockNatsClient::new(),
            &config,
        ),
        clock: FixedEpochClock::from_secs(TEST_NOW),
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    };
    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<AckFailPublisher, MockObjectStore, AdvancedMockNatsClient, FixedEpochClock>),
        )
        .with_state(state);
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let started = tokio::time::Instant::now();
    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(elapsed, std::time::Duration::from_millis(2800));
    assert!(elapsed < super::super::constants::SLACK_ACK_DEADLINE);
}

#[derive(Clone, Default)]
struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn capture_logs() -> (CapturedLogs, tracing::subscriber::DefaultGuard) {
    let logs = CapturedLogs::default();
    let guard = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .set_default();
    (logs, guard)
}

#[tokio::test]
async fn provision_warns_when_the_configured_ack_timeout_was_capped() {
    let (logs, _guard) = capture_logs();
    let mut config = test_config();
    config.ack_budget = AckBudget::new(Some(NonZeroDuration::from_secs(10).unwrap()));

    provision(&MockJetStreamContext::new(), &config).await.unwrap();

    let text = logs.text();
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("requested_secs=10"), "{text}");
    assert!(text.contains("effective_millis=2800"), "{text}");
}

#[tokio::test]
async fn provision_stays_quiet_when_the_ack_timeout_fits() {
    let (logs, _guard) = capture_logs();

    provision(&MockJetStreamContext::new(), &test_config()).await.unwrap();

    let text = logs.text();
    assert!(!text.contains("WARN"), "{text}");
}

fn unsigned_request(content_type: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/webhook")
        .header("content-type", content_type)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn ssl_check_is_answered_before_signature_verification_and_never_published() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(unsigned_request("application/x-www-form-urlencoded", "ssl_check=1"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn ssl_check_ignores_every_other_field() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let smuggled = "ssl_check=1&command=%2Ftrogon&team_id=T01ABC&trigger_id=cmd789";

    let resp = app
        .oneshot(unsigned_request("application/x-www-form-urlencoded", smuggled))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn ssl_check_with_a_valid_signature_is_still_not_published() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let form_body = "ssl_check=1&command=%2Ftrogon&team_id=T01ABC&trigger_id=cmd789";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

    let resp = app
        .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn requests_that_only_look_like_ssl_checks_still_need_a_signature() {
    let _guard = tracing_guard();
    let cases = [
        ("application/x-www-form-urlencoded", "ssl_check=0"),
        ("application/x-www-form-urlencoded", "ssl_check=1&ssl_check=1"),
        ("application/x-www-form-urlencoded", "command=%2Ftrogon"),
        ("application/x-www-form-urlencoded; charset=utf-8", "ssl_check=1"),
        ("application/json", "ssl_check=1"),
    ];
    for (content_type, body) in cases {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let resp = app.oneshot(unsigned_request(content_type, body)).await.unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{content_type} {body}");
        assert!(publisher.published_messages().is_empty());
    }
}

fn with_retry_headers(mut request: Request<Body>) -> Request<Body> {
    request.headers_mut().insert(HEADER_RETRY_NUM, "1".parse().unwrap());
    request
        .headers_mut()
        .insert(HEADER_RETRY_REASON, "http_timeout".parse().unwrap());
    request
}

fn assert_retry_headers(publisher: &MockJetStreamPublisher, expected: Option<(&str, &str)>) {
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    let num = messages[0].headers.get(NATS_HEADER_RETRY_NUM).map(|v| v.as_str());
    let reason = messages[0].headers.get(NATS_HEADER_RETRY_REASON).map(|v| v.as_str());
    assert_eq!(num.zip(reason), expected);
}

#[tokio::test]
async fn http_event_redelivery_carries_retry_headers() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = mock_app(publisher.clone())
        .oneshot(with_retry_headers(webhook_request(&body, &ts, Some(&sig))))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_retry_headers(&publisher, Some(("1", "http_timeout")));
}

#[tokio::test]
async fn http_first_delivery_has_no_retry_headers() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = event_callback_body("message");
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    assert_retry_headers(&publisher, None);
}

#[tokio::test]
async fn http_slash_command_redelivery_carries_retry_headers() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = b"command=%2Fdeploy&trigger_id=trigger123&team_id=T01ABC";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body);

    let resp = mock_app(publisher.clone())
        .oneshot(with_retry_headers(form_request(body, &ts, &sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_retry_headers(&publisher, Some(("1", "http_timeout")));
}

#[tokio::test]
async fn http_interaction_redelivery_carries_retry_headers() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let payload =
        serde_json::json!({ "type": "block_actions", "trigger_id": "trigger123", "team": { "id": "T01ABC" } });
    let body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(payload.to_string().as_bytes()).collect::<String>()
    );
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body.as_bytes());

    let resp = mock_app(publisher.clone())
        .oneshot(with_retry_headers(form_request(body.as_bytes(), &ts, &sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_retry_headers(&publisher, Some(("1", "http_timeout")));
}

fn enterprise_headers(publisher: &MockJetStreamPublisher) -> (Option<String>, Option<String>, Option<String>) {
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    let header = |name: &str| messages[0].headers.get(name).map(|v| v.as_str().to_owned());
    (
        header(NATS_HEADER_ENTERPRISE_ID),
        header(NATS_HEADER_IS_ENTERPRISE_INSTALL),
        header(NATS_HEADER_API_APP_ID),
    )
}

fn expected_enterprise(enterprise: Option<&str>, install: &str) -> (Option<String>, Option<String>, Option<String>) {
    (
        enterprise.map(str::to_owned),
        Some(install.to_owned()),
        Some("A01APP".to_owned()),
    )
}

#[tokio::test]
async fn http_event_carries_enterprise_and_app_identity() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "api_app_id": "A01APP",
        "authorizations": [{ "enterprise_id": "E01ENT", "is_enterprise_install": true }],
        "event": { "type": "message" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    assert_eq!(
        enterprise_headers(&publisher),
        expected_enterprise(Some("E01ENT"), "true")
    );
}

#[tokio::test]
async fn http_interaction_carries_enterprise_and_app_identity() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let payload = serde_json::json!({
        "type": "block_actions",
        "trigger_id": "trigger123",
        "api_app_id": "A01APP",
        "team": { "id": "T01ABC", "enterprise_id": "E01ENT" },
        "is_enterprise_install": false
    });
    let body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(payload.to_string().as_bytes()).collect::<String>()
    );
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body.as_bytes());

    mock_app(publisher.clone())
        .oneshot(form_request(body.as_bytes(), &ts, &sig))
        .await
        .unwrap();

    assert_eq!(
        enterprise_headers(&publisher),
        expected_enterprise(Some("E01ENT"), "false")
    );
}

#[tokio::test]
async fn http_slash_command_carries_enterprise_and_app_identity() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = b"command=%2Fdeploy&trigger_id=trigger123&team_id=T01ABC&api_app_id=A01APP&enterprise_id=E01ENT&is_enterprise_install=true";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body);

    mock_app(publisher.clone())
        .oneshot(form_request(body, &ts, &sig))
        .await
        .unwrap();

    assert_eq!(
        enterprise_headers(&publisher),
        expected_enterprise(Some("E01ENT"), "true")
    );
}

#[tokio::test]
async fn http_event_with_line_break_in_app_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "api_app_id": "A01\r\nX-Injected: 1",
        "event": { "type": "message" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn http_event_carries_subtype_and_channel_type() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "event": { "type": "message", "subtype": "bot_message", "channel_type": "im" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    let messages = publisher.published_messages();
    let header = |name: &str| messages[0].headers.get(name).map(|v| v.as_str());
    assert_eq!(header("X-Slack-Event-Subtype"), Some("bot_message"));
    assert_eq!(header("X-Slack-Channel-Type"), Some("im"));
}

#[tokio::test]
async fn http_event_with_line_break_in_subtype_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev01ABC123",
        "team_id": "T01ABC",
        "event": { "type": "message", "subtype": "bot\r\nX-Injected: 1" }
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn http_delivery_carries_the_request_timestamp_for_every_payload_kind() {
    let _guard = tracing_guard();
    let ts = current_timestamp();
    let interaction =
        serde_json::json!({ "type": "block_actions", "trigger_id": "trigger123", "team": { "id": "T01ABC" } });
    let interaction_body = format!(
        "payload={}",
        form_urlencoded::byte_serialize(interaction.to_string().as_bytes()).collect::<String>()
    );
    let command_body = "command=%2Fdeploy&trigger_id=trigger123&team_id=T01ABC".to_owned();
    let event_body = String::from_utf8(event_callback_body("message")).unwrap();

    for (body, form) in [(event_body, false), (interaction_body, true), (command_body, true)] {
        let publisher = MockJetStreamPublisher::new();
        let sig = compute_sig(TEST_SECRET, &ts, body.as_bytes());
        let request = if form {
            form_request(body.as_bytes(), &ts, &sig)
        } else {
            webhook_request(body.as_bytes(), &ts, Some(&sig))
        };

        mock_app(publisher.clone()).oneshot(request).await.unwrap();

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].headers.get("X-Slack-Request-Timestamp").map(|v| v.as_str()),
            Some(ts.as_str())
        );
        assert!(messages[0].headers.get("X-Slack-Envelope-Id").is_none());
    }
}

fn app_rate_limited_body() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "token": "verification-token",
        "team_id": "T01ABC",
        "minute_rate_limited": 1_518_467_820,
        "api_app_id": "A01ABC",
        "type": "app_rate_limited"
    }))
    .unwrap()
}

#[tokio::test]
async fn app_rate_limited_is_published_on_the_system_subject() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = app_rate_limited_body();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = app.oneshot(webhook_request(&body, &ts, Some(&sig))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.system.app_rate_limited");
    assert_eq!(messages[0].payload.as_ref(), body.as_slice());
    let headers = &messages[0].headers;
    assert_eq!(header(headers, NATS_HEADER_PAYLOAD_KIND), Some("system"));
    assert_eq!(header(headers, NATS_HEADER_EVENT_TYPE), Some("app_rate_limited"));
    assert_eq!(header(headers, NATS_HEADER_TEAM_ID), Some("T01ABC"));
    assert_eq!(header(headers, NATS_HEADER_TRANSPORT), Some("webhook"));
    assert_eq!(header(headers, NATS_HEADER_API_APP_ID), Some("A01ABC"));
    assert_eq!(header(headers, NATS_HEADER_EVENT_ID), None);
    assert_eq!(header(headers, NATS_HEADER_REQUEST_TIMESTAMP), Some(ts.as_str()));
    let expected_id = super::super::system_notice::SystemMessageId::new(&body);
    assert_eq!(header(headers, "Nats-Msg-Id"), Some(expected_id.as_str()));
}

#[tokio::test]
async fn a_redelivered_rate_limit_notice_keeps_its_message_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = app_rate_limited_body();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    for _ in 0..2 {
        mock_app(publisher.clone())
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();
    }

    let messages = publisher.published_messages();
    assert_eq!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        header(&messages[1].headers, "Nats-Msg-Id")
    );
}

#[tokio::test]
async fn app_rate_limited_without_a_team_id_uses_the_unknown_team() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = serde_json::to_vec(&serde_json::json!({ "type": "app_rate_limited" })).unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "slack.system.app_rate_limited");
    assert_eq!(header(&messages[0].headers, NATS_HEADER_TEAM_ID), Some("unknown"));
}

#[tokio::test]
async fn app_rate_limited_publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let body = app_rate_limited_body();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = mock_app(publisher)
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn app_rate_limited_with_unsafe_header_text_is_stored_as_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "app_rate_limited",
        "team_id": "T01\r\nX-Injected: 1"
    }))
    .unwrap();
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, &body);

    let resp = mock_app(publisher.clone())
        .oneshot(webhook_request(&body, &ts, Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
}

#[tokio::test]
async fn events_and_system_notices_are_labelled_as_json() {
    let _guard = tracing_guard();
    for body in [event_callback_body("message"), app_rate_limited_body()] {
        let publisher = MockJetStreamPublisher::new();
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        mock_app(publisher.clone())
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        let messages = publisher.published_messages();
        assert_eq!(header(&messages[0].headers, "Content-Type"), Some("application/json"));
    }
}

#[tokio::test]
async fn unroutable_messages_are_labelled_as_opaque_bytes() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let body = b"not json at all";
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body);

    mock_app(publisher.clone())
        .oneshot(webhook_request(body, &ts, Some(&sig)))
        .await
        .unwrap();

    let messages = publisher.published_messages();
    assert_eq!(
        header(&messages[0].headers, "Content-Type"),
        Some("application/octet-stream")
    );
}

fn signed_form(body: &str) -> Request<Body> {
    let ts = current_timestamp();
    let sig = compute_sig(TEST_SECRET, &ts, body.as_bytes());
    form_request(body.as_bytes(), &ts, &sig)
}

fn interaction_form(payload: &serde_json::Value) -> String {
    format!(
        "payload={}",
        form_urlencoded::byte_serialize(payload.to_string().as_bytes()).collect::<String>()
    )
}

fn view_submission_payload(callback_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "view_submission",
        "trigger_id": "trigger456",
        "team": { "id": "T01ABC" },
        "user": { "id": "U01ABC" },
        "view": { "callback_id": callback_id }
    })
}

fn reply_headers(content_type: &str) -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Content-Type", content_type);
    headers
}

async fn response_body(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()
}

const SLASH_FORM: &str = "command=%2Ftrogon&text=hello&team_id=T01ABC&trigger_id=cmd789&user_id=U01ABC";

#[tokio::test]
async fn view_submission_reply_becomes_the_http_body_byte_for_byte() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let requester = AdvancedMockNatsClient::new();
    let reply = br#"{ "response_action":"errors",  "errors":{"block":"bad"} }"#;
    requester.set_response_wire(
        "slack.respond.interaction.view_submission",
        reply_headers("application/json"),
        Bytes::from_static(reply),
    );
    let app = mock_app_with_requester(publisher.clone(), requester.clone());

    let form = interaction_form(&view_submission_payload("my_modal"));
    let response = app.oneshot(signed_form(&form)).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(response_body(response).await.as_ref(), reply);
    assert_eq!(
        publisher.published_subjects(),
        vec!["slack.interaction.view_submission"]
    );
    let headers = requester.requested_headers();
    assert_eq!(headers.len(), 1);
    assert_eq!(header(&headers[0], "X-Slack-Callback-Id"), Some("my_modal"));
    assert_eq!(header(&headers[0], NATS_HEADER_EVENT_TYPE), Some("view_submission"));
    assert_eq!(
        requester.requested_payloads(),
        vec![Bytes::from(view_submission_payload("my_modal").to_string())]
    );
}

#[tokio::test]
async fn view_submission_without_a_callback_id_sends_no_callback_header() {
    let _guard = tracing_guard();
    let requester = acknowledging_requester();
    let payload = serde_json::json!({
        "type": "view_submission",
        "trigger_id": "trigger456",
        "team": { "id": "T01ABC" },
        "view": {}
    });
    let response = mock_app_with_requester(MockJetStreamPublisher::new(), requester.clone())
        .oneshot(signed_form(&interaction_form(&payload)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&requester.requested_headers()[0], "X-Slack-Callback-Id"), None);
}

#[tokio::test]
async fn view_submission_with_an_unrepresentable_callback_id_is_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let requester = acknowledging_requester();
    let response = mock_app_with_requester(publisher.clone(), requester.clone())
        .oneshot(signed_form(&interaction_form(&view_submission_payload("bad\nid"))))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
    assert!(requester.requested_payloads().is_empty());
}

fn block_suggestion_payload() -> serde_json::Value {
    serde_json::json!({
        "type": "block_suggestion",
        "team": { "id": "T01ABC" },
        "action_id": "pick",
        "block_id": "blk",
        "value": "tro"
    })
}

#[tokio::test]
async fn block_suggestion_options_are_the_body_and_nothing_is_published() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let requester = AdvancedMockNatsClient::new();
    let options = br#"{"options":[{"text":{"type":"plain_text","text":"One"},"value":"1"}]}"#;
    requester.set_response_wire(
        "slack.respond.interaction.block_suggestion",
        reply_headers("application/json"),
        Bytes::from_static(options),
    );
    let response = mock_app_with_requester(publisher.clone(), requester.clone())
        .oneshot(signed_form(&interaction_form(&block_suggestion_payload())))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(response_body(response).await.as_ref(), options);
    assert!(publisher.published_messages().is_empty());
    let headers = requester.requested_headers();
    assert_eq!(headers.len(), 1);
    assert_eq!(header(&headers[0], "X-Slack-Action-Id"), Some("pick"));
    assert_eq!(header(&headers[0], "X-Slack-Block-Id"), Some("blk"));
    assert_eq!(header(&headers[0], "X-Slack-Callback-Id"), None);
    assert_eq!(header(&headers[0], NATS_HEADER_EVENT_TYPE), Some("block_suggestion"));
}

#[tokio::test]
async fn block_suggestion_with_no_consumer_is_a_404() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let response = mock_app_with_requester(publisher.clone(), AdvancedMockNatsClient::new())
        .oneshot(signed_form(&interaction_form(&block_suggestion_payload())))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test(start_paused = true)]
async fn block_suggestion_waits_for_the_consumer_until_the_ack_limit() {
    let _guard = tracing_guard();
    let requester = AdvancedMockNatsClient::new();
    requester.hang_next_request();
    let started = tokio::time::Instant::now();
    let response = mock_app_with_requester(MockJetStreamPublisher::new(), requester)
        .oneshot(signed_form(&interaction_form(&block_suggestion_payload())))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(started.elapsed(), crate::source::slack::constants::ACK_LIMIT_MAX);
}

#[tokio::test]
async fn block_suggestion_with_an_unrepresentable_identifier_is_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let requester = acknowledging_requester();
    let payload = serde_json::json!({
        "type": "block_suggestion",
        "team": { "id": "T01ABC" },
        "action_id": "bad\nid"
    });
    let response = mock_app_with_requester(publisher.clone(), requester.clone())
        .oneshot(signed_form(&interaction_form(&payload)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_unroutable(&publisher, "invalid_header_value");
    assert!(requester.requested_payloads().is_empty());
}

#[tokio::test]
async fn an_empty_consumer_reply_is_a_plain_acknowledgement() {
    let _guard = tracing_guard();
    let response = mock_app(MockJetStreamPublisher::new())
        .oneshot(signed_form(&interaction_form(&view_submission_payload("m"))))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_body(response).await.is_empty());
}

#[tokio::test]
async fn slash_command_reply_keeps_the_consumer_content_type() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let requester = AdvancedMockNatsClient::new();
    requester.set_response_wire(
        "slack.respond.command.trogon",
        reply_headers("text/plain"),
        Bytes::from_static(b"It is 80 degrees"),
    );
    let response = mock_app_with_requester(publisher.clone(), requester.clone())
        .oneshot(signed_form(SLASH_FORM))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/plain")
    );
    assert_eq!(response_body(response).await.as_ref(), b"It is 80 degrees");
    assert_eq!(publisher.published_subjects(), vec!["slack.command.trogon"]);
    assert_eq!(requester.requested_payloads(), vec![Bytes::from(SLASH_FORM)]);
    assert_eq!(header(&requester.requested_headers()[0], "X-Slack-Callback-Id"), None);
}

#[tokio::test]
async fn a_reply_without_a_content_type_is_json() {
    let _guard = tracing_guard();
    let requester = AdvancedMockNatsClient::new();
    requester.set_response("slack.respond.command.trogon", Bytes::from_static(b"plain"));
    let response = mock_app_with_requester(MockJetStreamPublisher::new(), requester)
        .oneshot(signed_form(SLASH_FORM))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(response_body(response).await.as_ref(), b"plain");
}

#[tokio::test]
async fn no_consumer_answering_a_slash_command_is_a_404() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let response = mock_app_with_requester(publisher.clone(), AdvancedMockNatsClient::new())
        .oneshot(signed_form(SLASH_FORM))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(publisher.published_subjects(), vec!["slack.command.trogon"]);
}

#[tokio::test]
async fn a_failed_request_to_a_view_submission_consumer_is_a_404() {
    let _guard = tracing_guard();
    let requester = acknowledging_requester();
    requester.fail_next_request();
    let response = mock_app_with_requester(MockJetStreamPublisher::new(), requester)
        .oneshot(signed_form(&interaction_form(&view_submission_payload("m"))))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(start_paused = true)]
async fn a_consumer_that_misses_the_deadline_gets_a_404() {
    let _guard = tracing_guard();
    let requester = acknowledging_requester();
    requester.hang_next_request();
    let started = tokio::time::Instant::now();
    let response = mock_app_with_requester(MockJetStreamPublisher::new(), requester)
        .oneshot(signed_form(SLASH_FORM))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(started.elapsed(), crate::source::slack::constants::ACK_LIMIT_MAX);
}

#[tokio::test]
async fn a_failed_publish_skips_the_consumer_request() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let requester = acknowledging_requester();
    let response = mock_app_with_requester(publisher, requester.clone())
        .oneshot(signed_form(SLASH_FORM))
        .await
        .unwrap();

    assert!(!response.status().is_success());
    assert!(requester.requested_payloads().is_empty());
}

#[tokio::test]
async fn interactions_that_need_no_answer_never_ask_a_consumer() {
    let _guard = tracing_guard();
    let requester = AdvancedMockNatsClient::new();
    let payload = serde_json::json!({ "type": "block_actions", "trigger_id": "t1", "team": { "id": "T01ABC" } });
    let response = mock_app_with_requester(MockJetStreamPublisher::new(), requester.clone())
        .oneshot(signed_form(&interaction_form(&payload)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(requester.requested_payloads().is_empty());
}
