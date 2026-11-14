#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use buffa::Message;
use bytes::Bytes;
use tower::ServiceExt;
use trogon_nats::DottedNatsToken;
use trogon_nats::jetstream::{
    ClaimBucket, ClaimCheckPublisher, ClaimResolver, ClaimRetention, MaxPayload, NatsJetStreamClient, NatsObjectStore,
    StreamMaxAge,
};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::{NonZeroDuration, SecretString};
use trogon_webhooks::{
    AuthenticationError, NatsEndpointRegistry, RegisteredEndpoint, SecretWebhookVerifier, WebhookSecretLookup, router,
};
use trogonai_proto::gateway::webhooks::v1;

#[derive(Clone)]
struct SenderSecret(SecretString);

impl WebhookSecretLookup for SenderSecret {
    async fn lookup_secret(&self, _: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        Ok(self.0.clone())
    }
}

fn registration(endpoint_id: &str) -> v1::EndpointRegistration {
    v1::EndpointRegistration {
        endpoint_id: endpoint_id.to_owned(),
        project_id: "project-a".to_owned(),
        status: v1::EndpointStatus::Enabled.into(),
        authentication_mode: v1::AuthenticationMode::Bearer.into(),
        max_body_bytes: 128 * 1024,
        timestamp_tolerance_seconds: 300,
    }
}

fn request(endpoint_id: &str, credential: &SecretString, body: &[u8]) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/webhooks/{endpoint_id}"))
        .header("authorization", format!("Bearer {}", credential.as_str()))
        .header("webhook-id", "native-delivery")
        .header("content-type", "application/octet-stream")
        .header("Nats-Msg-Id", "caller-cannot-select-this")
        .header("Trogon-Project-Id", "caller-cannot-select-this")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

async fn native_round_trip(server_payload_limit: usize, body: Vec<u8>) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let registry = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "WEBHOOK_ENDPOINTS".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut stream = js
        .create_stream(async_nats::jetstream::stream::Config {
            name: "WEBHOOK_DELIVERIES".to_owned(),
            subjects: vec!["webhooks.>".to_owned()],
            max_age: Duration::from_secs(604_800),
            duplicate_window: Duration::from_secs(604_800),
            ..Default::default()
        })
        .await
        .unwrap();
    let retention = ClaimRetention::tracking(
        StreamMaxAge::from_secs(604_800).unwrap(),
        NonZeroDuration::from_secs(3600).unwrap(),
    );
    let publisher_binding = NatsObjectStore::provision_claim_bucket(&js, ClaimBucket::default(), retention)
        .await
        .unwrap();
    let consumer_binding = NatsObjectStore::provision_claim_bucket(&js, ClaimBucket::default(), retention)
        .await
        .unwrap();
    let credential = SecretString::new(format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4())).unwrap();
    let endpoint_id = uuid::Uuid::new_v4().to_string();
    let app = router(
        NatsEndpointRegistry::new(registry.clone()),
        SecretWebhookVerifier::new(SenderSecret(credential.clone())),
        ClaimCheckPublisher::new(
            NatsJetStreamClient::new(js.clone()),
            publisher_binding,
            MaxPayload::from_server_limit(server_payload_limit),
        ),
        DottedNatsToken::new("webhooks").unwrap(),
        NonZeroDuration::from_secs(10).unwrap(),
    );

    let missing = app
        .clone()
        .oneshot(request(&endpoint_id, &credential, &body))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let mut registration = registration(&endpoint_id);
    let revision = registry
        .create(&endpoint_id, Bytes::from(registration.encode_to_vec()))
        .await
        .unwrap();
    let accepted = app
        .clone()
        .oneshot(request(&endpoint_id, &credential, &body))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let duplicate = app
        .clone()
        .oneshot(request(&endpoint_id, &credential, &body))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
    assert_eq!(stream.info().await.unwrap().state.messages, 1);

    let stored = stream.get_raw_message(1).await.unwrap();
    assert_eq!(
        stored.subject.as_str(),
        format!("webhooks.project-a.{endpoint_id}.received")
    );
    let message_id = stored.headers.get(async_nats::header::NATS_MESSAGE_ID).unwrap();
    assert_ne!(message_id.as_str(), "caller-cannot-select-this");
    assert!(stored.headers.get("authorization").is_none());
    assert!(stored.headers.get("Trogon-Project-Id").is_none());
    let payload = ClaimResolver::new(consumer_binding)
        .resolve(Some(&stored.headers), stored.payload)
        .await
        .unwrap();
    let received = v1::WebhookReceived::decode_from_slice(&payload).unwrap();
    assert_eq!(received.endpoint_id, endpoint_id);
    assert_eq!(received.project_id, "project-a");
    assert_eq!(received.configuration_revision, revision);
    assert_eq!(received.delivery_id, "native-delivery");
    assert_eq!(received.body, body);
    assert_eq!(received.content_type.as_deref(), Some("application/octet-stream"));
    assert!(received.received_at.is_set());

    registration.status = v1::EndpointStatus::Disabled.into();
    registry
        .update(&endpoint_id, Bytes::from(registration.encode_to_vec()), revision)
        .await
        .unwrap();
    let disabled = app
        .clone()
        .oneshot(request(&endpoint_id, &credential, &body))
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
    registry.delete(&endpoint_id).await.unwrap();
    let deleted = app.oneshot(request(&endpoint_id, &credential, &body)).await.unwrap();
    assert_eq!(deleted.status(), StatusCode::NOT_FOUND);
    assert_eq!(stream.info().await.unwrap().state.messages, 1);
}

#[tokio::test]
async fn native_registry_changes_and_jetstream_deduplication_work_without_remounting() {
    native_round_trip(1_048_576, b"{ \"source\" : \"external\" }\n".to_vec()).await;
}

#[tokio::test]
async fn native_claim_check_preserves_the_webhook_envelope_and_raw_body() {
    native_round_trip(8192, vec![b'x'; 24 * 1024]).await;
}
