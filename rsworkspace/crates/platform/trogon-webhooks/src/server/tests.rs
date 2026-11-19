#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::Request;
use buffa::Message as _;
use tower::ServiceExt as _;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding, MaxPayload, MockJetStreamPublisher, MockObjectStore};
use trogon_std::SecretString;

use super::*;
use crate::{
    AuthenticationMode, ConfigurationRevision, EndpointRegistration, ProjectId, RegisteredEndpoint, RegistryError,
    SecretWebhookVerifier, TimestampTolerance, WebhookBodySize, WebhookSecretLookup,
};

const ENDPOINT: &str = "67b82e3f-5c99-486b-b8a0-acdad9548e3d";

#[derive(Clone)]
struct Registry(RegisteredEndpoint);

impl EndpointLookup for Registry {
    async fn lookup(&self, _: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        Ok(Some(self.0.clone()))
    }
}

#[derive(Clone)]
struct Secrets(SecretString);

impl WebhookSecretLookup for Secrets {
    async fn lookup_secret(&self, _: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        Ok(self.0.clone())
    }
}

fn fixture(clock: fn() -> Result<Duration, SystemTimeError>) -> (Router, MockJetStreamPublisher, SecretString) {
    let endpoint = RegisteredEndpoint::new(
        EndpointRegistration::new(
            EndpointId::new(ENDPOINT).unwrap(),
            ProjectId::new("project-a").unwrap(),
            EndpointStatus::Enabled,
            AuthenticationMode::Bearer,
            WebhookBodySize::new(1024).unwrap(),
            TimestampTolerance::new(300).unwrap(),
        ),
        ConfigurationRevision::new(1).unwrap(),
    );
    let token = SecretString::new(format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4())).unwrap();
    let publisher = MockJetStreamPublisher::new();
    let app = router_with_receipt_clock(
        Registry(endpoint),
        SecretWebhookVerifier::new(Secrets(token.clone())),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
        clock,
    );
    (app, publisher, token)
}

fn request(token: &SecretString) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/webhooks/{ENDPOINT}"))
        .header("authorization", format!("Bearer {}", token.as_str()))
        .header("webhook-id", "clock-delivery")
        .body(Body::from("{}"))
        .unwrap()
}

#[tokio::test]
async fn receipt_clock_before_epoch_refuses_http_delivery_without_publication() {
    let (app, publisher, token) = fixture(|| (UNIX_EPOCH - Duration::from_secs(1)).duration_since(UNIX_EPOCH));
    let response = app.oneshot(request(&token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn receipt_clock_seconds_overflow_refuses_http_delivery_without_publication() {
    let (app, publisher, token) = fixture(|| Ok(Duration::from_secs(i64::MAX as u64 + 1)));
    let response = app.oneshot(request(&token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn receipt_clock_preserves_the_supplied_seconds_and_nanoseconds_in_the_event() {
    let (app, publisher, token) = fixture(|| Ok(Duration::new(123, 456)));
    let response = app.oneshot(request(&token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    let event = v1::WebhookReceived::decode_from_slice(&messages[0].payload).unwrap();
    let received_at = event.received_at.as_option().unwrap();
    assert_eq!(received_at.seconds, 123);
    assert_eq!(received_at.nanos, 456);
}
