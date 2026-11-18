#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use buffa::Message as _;
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;
use tower::ServiceExt as _;
use trogon_nats::DottedNatsToken;
use trogon_nats::jetstream::{
    ClaimBucket, ClaimBucketBinding, ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::{NonZeroDuration, SecretString};
use trogon_webhooks::{
    AuthenticationError, AuthenticationMode, ConfigurationRevision, EndpointId, EndpointLookup, EndpointRegistration,
    EndpointStatus, ProjectId, RegisteredEndpoint, RegistryError, SecretWebhookVerifier, TimestampTolerance,
    WebhookBodySize, WebhookSecretLookup, WebhookVerifier, router,
};
use trogonai_proto::gateway::webhooks::v1;

const ENDPOINT: &str = "67b82e3f-5c99-486b-b8a0-acdad9548e3d";
const OTHER_ENDPOINT: &str = "2296254b-3f32-40fa-9158-733e5cb6fb94";

#[derive(Clone, Default)]
struct Registry(Arc<Mutex<BTreeMap<EndpointId, RegisteredEndpoint>>>);

impl Registry {
    fn insert(&self, endpoint: RegisteredEndpoint) {
        self.0
            .lock()
            .unwrap()
            .insert(endpoint.registration().endpoint_id().clone(), endpoint);
    }
}

impl EndpointLookup for Registry {
    async fn lookup(&self, endpoint: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        Ok(self.0.lock().unwrap().get(endpoint).cloned())
    }
}

#[derive(Clone)]
struct Secrets(SecretString);

impl WebhookSecretLookup for Secrets {
    async fn lookup_secret(&self, _endpoint: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        Ok(self.0.clone())
    }
}

fn secret() -> SecretString {
    SecretString::new(format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    ))
    .unwrap()
}

fn endpoint(
    id: &str,
    mode: AuthenticationMode,
    status: EndpointStatus,
    limit: usize,
    revision: u64,
) -> RegisteredEndpoint {
    RegisteredEndpoint::new(
        EndpointRegistration::new(
            EndpointId::new(id).unwrap(),
            ProjectId::new("project-1").unwrap(),
            status,
            mode,
            WebhookBodySize::new(limit).unwrap(),
            TimestampTolerance::new(300).unwrap(),
        ),
        ConfigurationRevision::new(revision).unwrap(),
    )
}

fn app(registry: Registry, secret: SecretString, publisher: MockJetStreamPublisher) -> axum::Router {
    router(
        registry,
        SecretWebhookVerifier::new(Secrets(secret)),
        ClaimCheckPublisher::new(
            publisher,
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    )
}

fn request(id: &str, token: &SecretString, body: &[u8]) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/webhooks/{id}"))
        .header("authorization", format!("Bearer {}", token.as_str()))
        .header("webhook-id", "delivery-1")
        .header("content-type", "application/json")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn sign_headers(key: &[u8], body: &[u8], timestamp: &str) -> HeaderMap {
    let mut hmac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    hmac.update(b"delivery-1.");
    hmac.update(timestamp.as_bytes());
    hmac.update(b".");
    hmac.update(body);
    let signature = STANDARD.encode(hmac.finalize().into_bytes());
    let mut headers = HeaderMap::new();
    headers.insert("webhook-id", "delivery-1".parse().unwrap());
    headers.insert("webhook-timestamp", timestamp.parse().unwrap());
    headers.insert("webhook-signature", format!("v1,{signature}").parse().unwrap());
    headers
}

#[tokio::test]
async fn authenticated_delivery_publishes_raw_body_with_registry_owned_identity() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        7,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let body = b"{\n  \"project_id\": \"attacker-project\", \"id\": 5\n}\n";
    let mut input = request(ENDPOINT, &token, body);
    input
        .headers_mut()
        .insert("nats-msg-id", "attacker-id".parse().unwrap());
    input
        .headers_mut()
        .insert("x-tenant-id", "attacker-tenant".parse().unwrap());
    let response = app(registry, token, publisher.clone()).oneshot(input).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, format!("webhooks.project-1.{ENDPOINT}.received"));
    let event = v1::WebhookReceived::decode_from_slice(&messages[0].payload).unwrap();
    assert_eq!(event.endpoint_id, ENDPOINT);
    assert_eq!(event.project_id, "project-1");
    assert_eq!(event.configuration_revision, 7);
    assert_eq!(event.delivery_id, "delivery-1");
    assert_eq!(event.body.as_slice(), body);
    assert_eq!(event.content_type.as_deref(), Some("application/json"));
    assert!(event.received_at.as_option().is_some());
    assert!(messages[0].headers.get("authorization").is_none());
    assert!(messages[0].headers.get("x-tenant-id").is_none());
    assert_ne!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .unwrap()
            .as_str(),
        "attacker-id"
    );
}

#[tokio::test]
async fn existing_router_observes_endpoint_creation_update_and_disable() {
    let registry = Registry::default();
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry.clone(), token.clone(), publisher.clone());
    assert_eq!(
        router
            .clone()
            .oneshot(request(ENDPOINT, &token, b"{}"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    assert_eq!(
        router
            .clone()
            .oneshot(request(ENDPOINT, &token, b"{}"))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        1,
        2,
    ));
    assert_eq!(
        router
            .clone()
            .oneshot(request(ENDPOINT, &token, b"{}"))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Disabled,
        4096,
        3,
    ));
    assert_eq!(
        router.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(publisher.published_messages().len(), 1);
}

#[tokio::test]
async fn bearer_authentication_rejects_missing_wrong_and_duplicate_headers() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    let mut missing = request(ENDPOINT, &token, b"{}");
    missing.headers_mut().remove("authorization");
    let mut duplicate = request(ENDPOINT, &token, b"{}");
    duplicate
        .headers_mut()
        .append("authorization", "Bearer invalid".parse().unwrap());
    for input in [missing, request(ENDPOINT, &secret(), b"{}"), duplicate] {
        assert_eq!(
            router.clone().oneshot(input).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn signed_delivery_preserves_non_utf8_bytes_and_rejects_tampering() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::HmacSha256,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let key = uuid::Uuid::new_v4().as_bytes().repeat(2);
    let signing_secret = SecretString::new(format!("whsec_{}", STANDARD.encode(&key))).unwrap();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, signing_secret, publisher.clone());
    let body = b"raw\x00\xff\n";
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let headers = sign_headers(&key, body, &timestamp);
    for (payload, expected) in [
        (body.as_slice(), StatusCode::ACCEPTED),
        (b"tampered".as_slice(), StatusCode::UNAUTHORIZED),
    ] {
        let mut input = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/{ENDPOINT}"))
            .body(Body::from(payload.to_vec()))
            .unwrap();
        *input.headers_mut() = headers.clone();
        assert_eq!(router.clone().oneshot(input).await.unwrap().status(), expected);
    }
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        v1::WebhookReceived::decode_from_slice(&messages[0].payload)
            .unwrap()
            .body
            .as_slice(),
        body
    );
}

#[tokio::test]
async fn signed_delivery_rejects_reframing_the_body_as_part_of_the_delivery_id() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::HmacSha256,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let key = uuid::Uuid::new_v4().as_bytes().repeat(2);
    let signing_secret = SecretString::new(format!("whsec_{}", STANDARD.encode(&key))).unwrap();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, signing_secret, publisher.clone());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let original_body = format!("x.{timestamp}.changed");
    let forged_id = format!("delivery-1.{timestamp}.x");
    let forged_body = "changed";
    assert_eq!(
        format!("delivery-1.{timestamp}.{original_body}"),
        format!("{forged_id}.{timestamp}.{forged_body}")
    );
    let captured_headers = sign_headers(&key, original_body.as_bytes(), &timestamp);
    let mut original = Request::builder()
        .method("POST")
        .uri(format!("/webhooks/{ENDPOINT}"))
        .body(Body::from(original_body.clone()))
        .unwrap();
    *original.headers_mut() = captured_headers.clone();
    assert_eq!(
        router.clone().oneshot(original).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    let mut forged = Request::builder()
        .method("POST")
        .uri(format!("/webhooks/{ENDPOINT}"))
        .body(Body::from(forged_body))
        .unwrap();
    *forged.headers_mut() = captured_headers;
    forged.headers_mut().insert("webhook-id", forged_id.parse().unwrap());
    assert_eq!(router.oneshot(forged).await.unwrap().status(), StatusCode::BAD_REQUEST);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    let event = v1::WebhookReceived::decode_from_slice(&messages[0].payload).unwrap();
    assert_eq!(event.delivery_id, "delivery-1");
    assert_eq!(event.body.as_slice(), original_body.as_bytes());
}

#[tokio::test]
async fn signed_delivery_rejects_stale_future_and_duplicate_signature_headers() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::HmacSha256,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let key = uuid::Uuid::new_v4().as_bytes().repeat(2);
    let signing_secret = SecretString::new(format!("whsec_{}", STANDARD.encode(&key))).unwrap();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, signing_secret, publisher.clone());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut duplicate = sign_headers(&key, b"{}", &now.to_string());
    duplicate.append("webhook-signature", "v1,invalid".parse().unwrap());
    let mut duplicate_time = sign_headers(&key, b"{}", &now.to_string());
    duplicate_time.append("webhook-timestamp", now.to_string().parse().unwrap());
    for headers in [
        sign_headers(&key, b"{}", &(now - 1000).to_string()),
        sign_headers(&key, b"{}", &(now + 1000).to_string()),
        duplicate,
        duplicate_time,
    ] {
        let mut input = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/{ENDPOINT}"))
            .body(Body::from("{}"))
            .unwrap();
        *input.headers_mut() = headers;
        assert_eq!(
            router.clone().oneshot(input).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn delivery_identity_is_required_bounded_and_unambiguous() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    let mut missing = request(ENDPOINT, &token, b"{}");
    missing.headers_mut().remove("webhook-id");
    let mut duplicate = request(ENDPOINT, &token, b"{}");
    duplicate
        .headers_mut()
        .append("webhook-id", "another-id".parse().unwrap());
    let mut long = request(ENDPOINT, &token, b"{}");
    long.headers_mut()
        .insert("webhook-id", "a".repeat(129).parse().unwrap());
    let mut empty = request(ENDPOINT, &token, b"{}");
    empty.headers_mut().insert("webhook-id", "".parse().unwrap());
    let mut whitespace = request(ENDPOINT, &token, b"{}");
    whitespace
        .headers_mut()
        .insert("webhook-id", "contains space".parse().unwrap());
    let mut dotted = request(ENDPOINT, &token, b"{}");
    dotted.headers_mut().insert("webhook-id", "delivery.1".parse().unwrap());
    for input in [missing, duplicate, long, empty, whitespace, dotted] {
        assert_eq!(
            router.clone().oneshot(input).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn broker_deduplication_identity_is_stable_and_endpoint_scoped() {
    let registry = Registry::default();
    for id in [ENDPOINT, OTHER_ENDPOINT] {
        registry.insert(endpoint(
            id,
            AuthenticationMode::Bearer,
            EndpointStatus::Enabled,
            4096,
            1,
        ));
    }
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    for id in [ENDPOINT, ENDPOINT, OTHER_ENDPOINT] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(id, &token, b"{}"))
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
    }
    let messages = publisher.published_messages();
    let ids: Vec<_> = messages
        .iter()
        .map(|message| {
            message
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .unwrap()
                .as_str()
        })
        .collect();
    assert_eq!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
}

#[tokio::test]
async fn interrupted_request_bodies_are_retryable_and_never_published() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    for error in [
        std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
        std::io::Error::from(std::io::ErrorKind::ConnectionReset),
        std::io::Error::other("length limit exceeded"),
    ] {
        let mut input = request(ENDPOINT, &token, b"");
        *input.body_mut() = Body::from_stream(futures_util::stream::iter([
            Ok(bytes::Bytes::from_static(b"partial")),
            Err(error),
        ]));
        assert_eq!(
            router.clone().oneshot(input).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn streamed_request_bodies_preserve_the_configured_size_limit() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        8,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    let mut input = request(ENDPOINT, &token, b"");
    *input.body_mut() = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"1234")),
        Ok(bytes::Bytes::from_static(b"56789")),
    ]));
    assert_eq!(
        router.oneshot(input).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn broker_publication_failure_is_retryable_and_never_accepted() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let response = app(registry, token.clone(), publisher)
        .oneshot(request(ENDPOINT, &token, b"{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn route_accepts_post_only_and_rejects_noncanonical_endpoint_ids() {
    let publisher = MockJetStreamPublisher::new();
    let router = app(Registry::default(), secret(), publisher.clone());
    let input = Request::builder()
        .uri(format!("/webhooks/{ENDPOINT}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.clone().oneshot(input).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        router
            .oneshot(request("not-a-uuid", &secret(), b"{}"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(publisher.published_messages().is_empty());
}

#[derive(Clone)]
struct UnavailableRegistry;

impl EndpointLookup for UnavailableRegistry {
    async fn lookup(&self, _endpoint: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        Err(RegistryError::Unavailable)
    }
}

#[tokio::test]
async fn registry_failure_is_retryable_and_does_not_publish() {
    let publisher = MockJetStreamPublisher::new();
    let token = secret();
    let app = router(
        UnavailableRegistry,
        SecretWebhookVerifier::new(Secrets(token.clone())),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    assert_eq!(
        app.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(publisher.published_messages().is_empty());
}

#[derive(Clone)]
struct StalledRegistry;

impl EndpointLookup for StalledRegistry {
    async fn lookup(&self, _endpoint: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn admission_timeout_covers_registry_lookup() {
    let publisher = MockJetStreamPublisher::new();
    let token = secret();
    let app = router(
        StalledRegistry,
        SecretWebhookVerifier::new(Secrets(token.clone())),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    let started = tokio::time::Instant::now();
    assert_eq!(
        app.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(5));
    assert!(publisher.published_messages().is_empty());
}

#[derive(Clone)]
struct UnavailableSecrets;

impl WebhookSecretLookup for UnavailableSecrets {
    async fn lookup_secret(&self, _endpoint: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        Err(AuthenticationError::Unavailable)
    }
}

#[derive(Clone)]
struct UnavailableSecretsWithSource;

impl WebhookSecretLookup for UnavailableSecretsWithSource {
    async fn lookup_secret(&self, _endpoint: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        Err(AuthenticationError::UnavailableWithSource(Box::new(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "test credential backend failure"),
        )))
    }
}

#[tokio::test]
async fn credential_service_source_errors_remain_private_retryable_and_never_publish() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let publisher = MockJetStreamPublisher::new();
    let app = router(
        registry,
        SecretWebhookVerifier::new(UnavailableSecretsWithSource),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    let response = app.oneshot(request(ENDPOINT, &secret(), b"{}")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn credential_service_unavailability_is_retryable_and_does_not_publish() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let publisher = MockJetStreamPublisher::new();
    let app = router(
        registry,
        SecretWebhookVerifier::new(UnavailableSecrets),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    assert_eq!(
        app.oneshot(request(ENDPOINT, &secret(), b"{}")).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(publisher.published_messages().is_empty());
}

#[derive(Clone, Copy)]
enum AckBehavior {
    Error,
    Stalled,
}

#[derive(Clone)]
struct AckPublisher(AckBehavior);

impl trogon_nats::jetstream::JetStreamPublisher for AckPublisher {
    type PublishError = std::io::Error;
    type AckFuture = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>
                + Send,
        >,
    >;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: bytes::Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        let behavior = self.0;
        Ok(Box::pin(async move {
            match behavior {
                AckBehavior::Error => Err(std::io::Error::other("acknowledgement rejected")),
                AckBehavior::Stalled => std::future::pending().await,
            }
        }))
    }
}

#[tokio::test(start_paused = true)]
async fn acceptance_requires_successful_broker_acknowledgement_within_admission_budget() {
    for behavior in [AckBehavior::Error, AckBehavior::Stalled] {
        let registry = Registry::default();
        registry.insert(endpoint(
            ENDPOINT,
            AuthenticationMode::Bearer,
            EndpointStatus::Enabled,
            4096,
            1,
        ));
        let token = secret();
        let app = router(
            registry,
            SecretWebhookVerifier::new(Secrets(token.clone())),
            ClaimCheckPublisher::new(
                AckPublisher(behavior),
                ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            DottedNatsToken::new("webhooks").unwrap(),
            NonZeroDuration::from_secs(5).unwrap(),
        );
        let started = tokio::time::Instant::now();
        assert_eq!(
            app.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        if matches!(behavior, AckBehavior::Stalled) {
            assert_eq!(started.elapsed(), std::time::Duration::from_secs(5));
        }
    }
}

#[tokio::test]
async fn body_and_content_type_have_hard_bounds_before_publication() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        trogon_webhooks::MAX_BODY_BYTES,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    let huge_body = request(ENDPOINT, &token, &vec![b'x'; trogon_webhooks::MAX_BODY_BYTES + 1]);
    assert_eq!(
        router.clone().oneshot(huge_body).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut huge_type = request(ENDPOINT, &token, b"{}");
    huge_type
        .headers_mut()
        .insert("content-type", "x".repeat(257).parse().unwrap());
    assert_eq!(
        router.oneshot(huge_type).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn malformed_bearer_credentials_are_rejected_without_publishing() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, token.clone(), publisher.clone());
    for authorization in ["Basic invalid", "Bearer ", "Bearer invalid extra", "missing-scheme"] {
        let mut input = request(ENDPOINT, &token, b"{}");
        input
            .headers_mut()
            .insert("authorization", authorization.parse().unwrap());
        assert_eq!(
            router.clone().oneshot(input).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(publisher.published_messages().is_empty());
}

#[derive(Clone)]
struct RotatingSecrets(Arc<Mutex<SecretString>>);

impl RotatingSecrets {
    fn replace(&self, value: SecretString) {
        *self.0.lock().unwrap() = value;
    }
}

impl WebhookSecretLookup for RotatingSecrets {
    async fn lookup_secret(&self, _endpoint: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn verifier_observes_credential_rotation_without_being_recreated() {
    let original = secret();
    let replacement = secret();
    let verifier = SecretWebhookVerifier::new(RotatingSecrets(Arc::new(Mutex::new(original.clone()))));
    let registered = endpoint(ENDPOINT, AuthenticationMode::Bearer, EndpointStatus::Enabled, 4096, 1);
    let old_request = request(ENDPOINT, &original, b"{}");
    assert!(verifier.verify(&registered, old_request.headers(), b"{}").await.is_ok());
    verifier.lookup().replace(replacement.clone());
    assert!(matches!(
        verifier.verify(&registered, old_request.headers(), b"{}").await,
        Err(AuthenticationError::Unauthorized)
    ));
    let mut new_request = request(ENDPOINT, &replacement, b"{}");
    new_request.headers_mut().insert(
        "authorization",
        format!("bearer {}", replacement.as_str()).parse().unwrap(),
    );
    assert!(verifier.verify(&registered, new_request.headers(), b"{}").await.is_ok());
}

#[tokio::test]
async fn malformed_signed_timestamps_are_rejected_without_publishing() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::HmacSha256,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let key = uuid::Uuid::new_v4().as_bytes().repeat(2);
    let signing_secret = SecretString::new(format!("whsec_{}", STANDARD.encode(&key))).unwrap();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, signing_secret, publisher.clone());
    for timestamp in ["", "not-a-timestamp", "+1", "18446744073709551616"] {
        let mut input = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/{ENDPOINT}"))
            .body(Body::from("{}"))
            .unwrap();
        *input.headers_mut() = sign_headers(&key, b"{}", timestamp);
        assert_eq!(
            router.clone().oneshot(input).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn signed_delivery_skips_unsupported_and_malformed_candidates_but_requires_a_valid_signature() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::HmacSha256,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let key = uuid::Uuid::new_v4().as_bytes().repeat(2);
    let signing_secret = SecretString::new(format!("whsec_{}", STANDARD.encode(&key))).unwrap();
    let publisher = MockJetStreamPublisher::new();
    let router = app(registry, signing_secret, publisher.clone());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let mut headers = sign_headers(&key, b"{}", &timestamp);
    let valid = headers.get("webhook-signature").unwrap().to_str().unwrap().to_owned();
    for (signatures, expected) in [
        ("v2,unsupported v1,%%%".to_owned(), StatusCode::UNAUTHORIZED),
        (format!("v2,unsupported v1,%%% {valid}"), StatusCode::ACCEPTED),
    ] {
        headers.insert("webhook-signature", signatures.parse().unwrap());
        let mut input = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/{ENDPOINT}"))
            .body(Body::from("{}"))
            .unwrap();
        *input.headers_mut() = headers.clone();
        assert_eq!(router.clone().oneshot(input).await.unwrap().status(), expected);
    }
    assert_eq!(publisher.published_messages().len(), 1);
}

#[tokio::test]
async fn misconfigured_hmac_credentials_fail_closed_as_service_unavailable() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::HmacSha256,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let key = uuid::Uuid::new_v4().as_bytes().repeat(2);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    for credential in [
        secret(),
        SecretString::new("whsec_%%%").unwrap(),
        SecretString::new(format!("whsec_{}", STANDARD.encode(&key[..31]))).unwrap(),
    ] {
        let publisher = MockJetStreamPublisher::new();
        let router = app(registry.clone(), credential, publisher.clone());
        let mut input = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/{ENDPOINT}"))
            .body(Body::from("{}"))
            .unwrap();
        *input.headers_mut() = sign_headers(&key, b"{}", &timestamp);
        assert_eq!(
            router.oneshot(input).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(publisher.published_messages().is_empty());
    }
}

#[derive(Clone)]
struct WrongEndpointRegistry;

impl EndpointLookup for WrongEndpointRegistry {
    async fn lookup(&self, _endpoint_id: &EndpointId) -> Result<Option<RegisteredEndpoint>, RegistryError> {
        Ok(Some(endpoint(
            OTHER_ENDPOINT,
            AuthenticationMode::Bearer,
            EndpointStatus::Enabled,
            4096,
            1,
        )))
    }
}

#[tokio::test]
async fn registry_cannot_redirect_a_request_to_a_different_endpoint() {
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = router(
        WrongEndpointRegistry,
        SecretWebhookVerifier::new(Secrets(token.clone())),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    assert_eq!(
        router.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn composed_subject_rejects_control_characters_before_publication() {
    let registry = Registry::default();
    registry.insert(endpoint(
        ENDPOINT,
        AuthenticationMode::Bearer,
        EndpointStatus::Enabled,
        4096,
        1,
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = router(
        registry,
        SecretWebhookVerifier::new(Secrets(token.clone())),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new("webhooks\0").unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    assert_eq!(
        router.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn longest_supported_subject_components_fit_and_preserve_routing() {
    let registry = Registry::default();
    let project = "p".repeat(64);
    let prefix = "w".repeat(128);
    registry.insert(RegisteredEndpoint::new(
        EndpointRegistration::new(
            EndpointId::new(ENDPOINT).unwrap(),
            ProjectId::new(&project).unwrap(),
            EndpointStatus::Enabled,
            AuthenticationMode::Bearer,
            WebhookBodySize::new(4096).unwrap(),
            TimestampTolerance::new(300).unwrap(),
        ),
        ConfigurationRevision::new(1).unwrap(),
    ));
    let token = secret();
    let publisher = MockJetStreamPublisher::new();
    let router = router(
        registry,
        SecretWebhookVerifier::new(Secrets(token.clone())),
        ClaimCheckPublisher::new(
            publisher.clone(),
            ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        DottedNatsToken::new(&prefix).unwrap(),
        NonZeroDuration::from_secs(5).unwrap(),
    );
    assert_eq!(
        router.oneshot(request(ENDPOINT, &token, b"{}")).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, format!("{prefix}.{project}.{ENDPOINT}.received"));
    assert!(messages[0].subject.len() <= trogon_nats::MAX_SUBJECT_BYTES);
}
