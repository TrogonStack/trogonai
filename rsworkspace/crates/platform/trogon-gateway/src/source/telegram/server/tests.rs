use super::*;
use axum::body::Body;
use axum::http::Request;
use std::time::Duration;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::NonZeroDuration;

use crate::credential::commands::domain::{CredentialOwnerId, CredentialRef, CredentialScope};
use crate::credential_runtime_projection::{RuntimeCredentialRegistry, RuntimeIntegrationProjection};
use crate::secret_store::{
    MockOpenBaoSecretStore, SecretDestroyReason, SecretStoreDestroy, SecretStorePut, SecretStoreRevoke,
    SecretStoreRotate,
};

const TEST_SECRET: &str = "test-secret";

fn test_config() -> TelegramSourceConfig {
    TelegramSourceConfig {
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        registration: None,
        subject_prefix: NatsToken::new("telegram").unwrap(),
        stream_name: NatsToken::new("TELEGRAM").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    }
}

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    router(wrap_publisher(publisher), &test_config())
}

struct RuntimeFixture {
    publisher: MockJetStreamPublisher,
    store: MockOpenBaoSecretStore,
    registry: RuntimeCredentialRegistry,
    integration_id: SourceIntegrationId,
    registered_credentials: TelegramRegisteredWebhookCredentials,
}

impl RuntimeFixture {
    fn new(publisher: MockJetStreamPublisher) -> Self {
        Self {
            publisher,
            store: MockOpenBaoSecretStore::default(),
            registry: RuntimeCredentialRegistry::default(),
            integration_id: SourceIntegrationId::new("primary").unwrap(),
            registered_credentials: TelegramRegisteredWebhookCredentials::default(),
        }
    }

    async fn put(&self, secret: &str) -> CredentialRef {
        self.store
            .put(
                CredentialScope::integration(
                    CredentialOwnerId::new("tenant-1").unwrap(),
                    SourceKind::Telegram,
                    self.integration_id.clone(),
                ),
                CredentialKind::WebhookSecret,
                trogon_std::SecretString::new(secret).unwrap(),
            )
            .await
            .unwrap()
    }

    async fn activate(&self, credential: CredentialRef, version: u64) {
        self.registry
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, version).unwrap())
            .await;
    }

    fn app(&self) -> Router {
        runtime_router(
            wrap_publisher(self.publisher.clone()),
            &test_config(),
            self.integration_id.clone(),
            self.registry.resolver(self.store.clone()),
            self.registered_credentials.clone(),
        )
    }

    fn register(&self, credential: CredentialRef) {
        self.registered_credentials
            .record(self.integration_id.clone(), credential);
    }
}

async fn runtime_app(publisher: MockJetStreamPublisher, secret: &str) -> Router {
    let fixture = RuntimeFixture::new(publisher);
    let credential = fixture.put(secret).await;
    fixture.activate(credential, 1).await;
    fixture.app()
}

fn webhook_request(body: &[u8], secret: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/webhook");

    if let Some(s) = secret {
        builder = builder.header(HEADER_SECRET_TOKEN, s);
    }

    builder.body(Body::from(body.to_vec())).unwrap()
}

fn update_json(update_type: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "update_id": 12345,
        update_type: {"chat": {"id": 1}}
    }))
    .unwrap()
}

#[tokio::test]
async fn provision_creates_stream() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    let config = test_config();

    provision(&js, &config).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "TELEGRAM");
    assert_eq!(streams[0].subjects, vec!["telegram.>"]);
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
async fn valid_message_update_publishes_to_nats() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "telegram.message");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_UPDATE_TYPE).map(|v| v.as_str()),
        Some("message"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_UPDATE_ID).map(|v| v.as_str()),
        Some("12345"),
    );
}

#[tokio::test]
async fn runtime_webhook_secret_publishes_to_nats_and_rejects_static_secret() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = runtime_app(publisher.clone(), "runtime-secret").await;
    let body = update_json("message");

    let static_response = app
        .clone()
        .oneshot(webhook_request(&body, Some(TEST_SECRET)))
        .await
        .unwrap();

    assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());

    let runtime_response = app
        .oneshot(webhook_request(&body, Some("runtime-secret")))
        .await
        .unwrap();

    assert_eq!(runtime_response.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "telegram.message");
}

async fn status_of(app: &Router, body: &[u8], token: &str) -> StatusCode {
    app.clone()
        .oneshot(webhook_request(body, Some(token)))
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn runtime_webhook_accepts_the_registered_secret_until_the_rotated_one_is_registered() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let fixture = RuntimeFixture::new(publisher.clone());
    let previous = fixture.put("previous-secret").await;
    let rotated = fixture
        .store
        .rotate(&previous, trogon_std::SecretString::new("rotated-secret").unwrap())
        .await
        .unwrap();
    fixture.activate(rotated.clone(), 2).await;
    fixture.register(previous);
    let app = fixture.app();
    let body = update_json("message");

    assert_eq!(status_of(&app, &body, "previous-secret").await, StatusCode::OK);
    assert_eq!(status_of(&app, &body, "rotated-secret").await, StatusCode::OK);
    assert_eq!(status_of(&app, &body, "unknown-secret").await, StatusCode::UNAUTHORIZED);
    assert_eq!(publisher.published_messages().len(), 2);

    fixture.register(rotated);

    assert_eq!(
        status_of(&app, &body, "previous-secret").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(status_of(&app, &body, "rotated-secret").await, StatusCode::OK);
}

#[tokio::test]
async fn runtime_webhook_rejects_a_revoked_registered_secret_when_the_replacement_is_not_registered() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let fixture = RuntimeFixture::new(publisher.clone());
    let revoked = fixture.put("revoked-secret").await;
    fixture.register(revoked.clone());
    fixture.store.revoke(&revoked).await.unwrap();
    fixture
        .store
        .destroy(&revoked, &SecretDestroyReason::new("revoked by operator").unwrap())
        .await
        .unwrap();
    let replacement = fixture.put("replacement-secret").await;
    fixture.activate(replacement, 2).await;
    let app = fixture.app();
    let body = update_json("message");

    assert_eq!(status_of(&app, &body, "revoked-secret").await, StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
    assert_eq!(status_of(&app, &body, "replacement-secret").await, StatusCode::OK);
    assert_eq!(publisher.published_messages().len(), 1);
}

#[tokio::test]
async fn callback_query_routes_to_correct_subject() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = update_json("callback_query");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["telegram.callback_query"]);
}

#[tokio::test]
async fn unknown_update_type_routes_to_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"update_id": 99}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["telegram.unroutable"]);
}

#[tokio::test]
async fn missing_update_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"message": {"chat": {"id": 1}}}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn non_integer_update_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"update_id": "not_a_number", "message": {"chat": {"id": 1}}}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_secret_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(webhook_request(b"{}", None)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn wrong_secret_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(webhook_request(b"{}", Some("wrong-secret"))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn invalid_json_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(webhook_request(b"not-json", Some(TEST_SECRET)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn subject_uses_configured_prefix() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("custom").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .with_state(state);

    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["custom.message"]);
}

#[tokio::test]
async fn update_id_used_as_nats_message_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("12345"),
    );
}

#[tokio::test]
async fn body_exceeding_limit_returns_413() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .layer(DefaultBodyLimit::max(64))
        .with_state(state);

    let oversized_body = vec![0u8; 128];
    let resp = app
        .oneshot(webhook_request(&oversized_body, Some(TEST_SECRET)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(publisher.published_messages().is_empty());
}

#[test]
fn extract_update_type_finds_known_types() {
    for &t in UPDATE_TYPES {
        let val = serde_json::json!({"update_id": 1, t: {}});
        assert_eq!(extract_update_type(&val), Some(t), "failed for type: {t}");
    }
}

#[test]
fn extract_update_type_returns_none_for_unknown() {
    let val = serde_json::json!({"update_id": 1});
    assert_eq!(extract_update_type(&val), None);
}

#[test]
fn extract_update_type_returns_none_for_non_object() {
    let val = serde_json::json!("not an object");
    assert_eq!(extract_update_type(&val), None);
}

mod ack_test_support;

use ack_test_support::AckFailPublisher;

#[tokio::test]
async fn ack_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = AckFailPublisher::failing();

    let state = AppState {
        publisher: ClaimCheckPublisher::new(
            publisher,
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn ack_timeout_returns_500() {
    let _guard = tracing_guard();
    let publisher = AckFailPublisher::hanging();

    let state = AppState {
        publisher: ClaimCheckPublisher::new(
            publisher,
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_millis(10).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
