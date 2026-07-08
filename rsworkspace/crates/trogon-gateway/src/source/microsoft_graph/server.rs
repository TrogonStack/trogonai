use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, RawQuery, State},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut};
use trogon_std::NonZeroDuration;

use super::client_state::MicrosoftGraphClientState;
use super::config::MicrosoftGraphConfig;
use super::constants::HTTP_BODY_SIZE_MAX;
use crate::credential::commands::domain::{CredentialKind, SourceKind};
use crate::credential::processor::runtime_projection::{
    RuntimeCredentialError, RuntimeCredentialResolver, RuntimeIntegrationKey,
};
use crate::secret_store::{SecretStoreError, SecretStoreGet};
use crate::source_integration_id::SourceIntegrationId;

#[derive(Deserialize)]
struct ChangeNotificationCollection {
    value: Vec<Value>,
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    client_state: MicrosoftGraphClientState,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

#[derive(Clone)]
struct RuntimeAppState<P: JetStreamPublisher, S: ObjectStorePut, G> {
    publisher: ClaimCheckPublisher<P, S>,
    credential_resolver: RuntimeCredentialResolver<G>,
    runtime_key: RuntimeIntegrationKey,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &MicrosoftGraphConfig) -> Result<(), C::Error> {
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
    config: &MicrosoftGraphConfig,
) -> Router {
    let state = AppState {
        publisher,
        client_state: config.client_state.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

pub fn runtime_router<P, S, G>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &MicrosoftGraphConfig,
    integration_id: SourceIntegrationId,
    credential_resolver: RuntimeCredentialResolver<G>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let state = RuntimeAppState {
        publisher,
        credential_resolver,
        runtime_key: RuntimeIntegrationKey::new(SourceKind::MicrosoftGraph, &integration_id),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_runtime_webhook::<P, S, G>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    if let Some(validation_token) = validation_token_from_query(query.as_deref()) {
        info!("Responding to Microsoft Graph notification URL validation");
        return (StatusCode::OK, [(CONTENT_TYPE, "text/plain")], validation_token).into_response();
    }

    publish_notification_collection(
        state.publisher,
        state.client_state,
        state.subject_prefix,
        state.nats_ack_timeout,
        body,
    )
    .await
    .into_response()
}

async fn handle_runtime_webhook<P, S, G>(
    State(state): State<RuntimeAppState<P, S, G>>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    if let Some(validation_token) = validation_token_from_query(query.as_deref()) {
        info!("Responding to Microsoft Graph notification URL validation");
        return (StatusCode::OK, [(CONTENT_TYPE, "text/plain")], validation_token).into_response();
    }

    let client_state = match state
        .credential_resolver
        .resolve_plaintext(&state.runtime_key, CredentialKind::ClientState)
        .await
    {
        Ok(secret) => secret,
        Err(error) => {
            warn!(reason = %error, "Microsoft Graph runtime credential resolution failed");
            return runtime_credential_error_to_status(&error).into_response();
        }
    };
    let client_state = match MicrosoftGraphClientState::new(client_state.as_str()) {
        Ok(client_state) => client_state,
        Err(error) => {
            warn!(reason = %error, "Microsoft Graph runtime client state is invalid");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    publish_notification_collection(
        state.publisher,
        client_state,
        state.subject_prefix,
        state.nats_ack_timeout,
        body,
    )
    .await
    .into_response()
}

#[instrument(
    name = "microsoft_graph.webhook",
    skip_all,
    fields(
        notification_count = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn publish_notification_collection<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    client_state: MicrosoftGraphClientState,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
    body: Bytes,
) -> StatusCode {
    let collection = match serde_json::from_slice::<ChangeNotificationCollection>(&body) {
        Ok(collection) => collection,
        Err(error) => {
            warn!(error = %error, "Failed to parse Microsoft Graph notification body as JSON");
            return StatusCode::BAD_REQUEST;
        }
    };

    if collection.value.is_empty() {
        warn!("Microsoft Graph notification body contained no notifications");
        return StatusCode::BAD_REQUEST;
    }

    for notification in &collection.value {
        match client_state_from_notification(notification) {
            Some(provided) if client_state.matches(provided) => {}
            Some(_) => {
                warn!("Microsoft Graph clientState validation failed");
                return StatusCode::UNAUTHORIZED;
            }
            None => {
                warn!("Microsoft Graph notification missing clientState");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    let subject = format!("{}.change_notification_collection", subject_prefix);
    let span = tracing::Span::current();
    span.record("notification_count", collection.value.len());
    span.record("subject", &subject);

    let message_id = change_notification_collection_message_id(&collection.value);
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(async_nats::header::NATS_MESSAGE_ID, message_id.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, nats_ack_timeout.into())
        .await;

    if outcome.is_ok() {
        info!("Published Microsoft Graph notification collection to NATS");
        StatusCode::ACCEPTED
    } else {
        outcome.log_on_error("microsoft-graph");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn runtime_credential_error_to_status(error: &RuntimeCredentialError) -> StatusCode {
    match error {
        RuntimeCredentialError::SecretStore(SecretStoreError::BackendUnavailable { .. }) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        RuntimeCredentialError::SecretStore(_)
        | RuntimeCredentialError::IntegrationNotFound { .. }
        | RuntimeCredentialError::IntegrationNotResolvable { .. }
        | RuntimeCredentialError::CredentialMissing { .. }
        | RuntimeCredentialError::VerifierOnly { .. } => StatusCode::UNAUTHORIZED,
    }
}

fn client_state_from_notification(notification: &Value) -> Option<&str> {
    notification.get("clientState").and_then(Value::as_str)
}

fn change_notification_collection_message_id(notifications: &[Value]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"microsoft-graph.change_notification_collection.v1");

    for notification in notifications {
        hasher.update(b"\x1fnotification\x1e");
        if let Some(subscription_id) = notification.get("subscriptionId").and_then(Value::as_str) {
            hasher.update(b"subscriptionId\x1e");
            hasher.update(subscription_id.as_bytes());
        }
        if let Some(id) = notification.get("id").and_then(Value::as_str) {
            hasher.update(b"id\x1e");
            hasher.update(id.as_bytes());
        } else {
            hasher.update(b"payload\x1e");
            hasher.update(serde_json::to_vec(notification).expect("serde_json::Value serialization should not fail"));
        }
    }

    hex::encode(hasher.finalize())
}

fn validation_token_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    form_urlencoded::parse(query.as_bytes()).find_map(|(key, value)| {
        if key == "validationToken" && !value.is_empty() {
            Some(value.into_owned())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::commands::domain::{CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
    use crate::credential::processor::runtime_projection::{RuntimeCredentialRegistry, RuntimeIntegrationProjection};
    use crate::secret_store::{MockOpenBaoSecretStore, SecretStorePut};
    use crate::source_integration_id::SourceIntegrationId;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
    };

    const TEST_CLIENT_STATE: &str = "secret-client-state";
    const RUNTIME_CLIENT_STATE: &str = "runtime-client-state";

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

    fn test_config() -> MicrosoftGraphConfig {
        MicrosoftGraphConfig {
            client_state: MicrosoftGraphClientState::new(TEST_CLIENT_STATE).unwrap(),
            subject_prefix: NatsToken::new("microsoft-graph").unwrap(),
            stream_name: NatsToken::new("MICROSOFT_GRAPH").unwrap(),
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

    async fn runtime_app(publisher: MockJetStreamPublisher, client_state: &str) -> Router {
        let config = test_config();
        let store = MockOpenBaoSecretStore::default();
        let integration_id = SourceIntegrationId::new("primary").unwrap();
        let credential = store
            .put(
                CredentialScope::integration(
                    CredentialOwnerId::new("tenant-1").unwrap(),
                    SourceKind::MicrosoftGraph,
                    integration_id.clone(),
                ),
                CredentialKind::ClientState,
                trogon_std::SecretString::new(client_state).unwrap(),
            )
            .await
            .unwrap();
        let registry = RuntimeCredentialRegistry::default();
        registry
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, 1).unwrap())
            .await;

        runtime_router(
            wrap_publisher(publisher),
            &config,
            integration_id,
            registry.resolver(store),
        )
    }

    fn webhook_request(uri: &str, body: impl Into<Body>) -> Request<Body> {
        Request::builder().method("POST").uri(uri).body(body.into()).unwrap()
    }

    fn notification_value() -> Value {
        notification_value_with_client_state(TEST_CLIENT_STATE)
    }

    fn notification_value_with_client_state(client_state: &str) -> Value {
        serde_json::json!({
            "id": "notification-1",
            "subscriptionId": "subscription-1",
            "subscriptionExpirationDateTime": "2026-04-28T17:00:00Z",
            "clientState": client_state,
            "changeType": "created",
            "tenantId": "tenant-1",
            "resource": "users('user-1')/messages('message-1')",
            "resourceData": {
                "id": "message-1",
                "@odata.type": "#Microsoft.Graph.message",
                "@odata.id": "users('user-1')/messages('message-1')"
            }
        })
    }

    fn collection_body(notification: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "value": [notification] })).unwrap()
    }

    fn assert_no_publishes(publisher: &MockJetStreamPublisher) {
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "MICROSOFT_GRAPH");
        assert_eq!(streams[0].subjects, vec!["microsoft-graph.>"]);
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
    async fn validation_token_challenge_returns_plain_text_token() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let response = app
            .oneshot(webhook_request("/webhook?validationToken=hello%20world", Body::empty()))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), b"hello world");
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn runtime_client_state_publishes_and_rejects_static_client_state() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = runtime_app(publisher.clone(), RUNTIME_CLIENT_STATE).await;

        let static_response = app
            .clone()
            .oneshot(webhook_request(
                "/webhook",
                collection_body(notification_value_with_client_state(TEST_CLIENT_STATE)),
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);

        let runtime_response = app
            .oneshot(webhook_request(
                "/webhook",
                collection_body(notification_value_with_client_state(RUNTIME_CLIENT_STATE)),
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-graph.change_notification_collection");
    }

    #[tokio::test]
    async fn valid_collection_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let notification = notification_value();
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body.clone())).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-graph.change_notification_collection");
        let expected_payload = serde_json::from_slice::<Value>(&body).unwrap();
        let expected_message_id =
            change_notification_collection_message_id(expected_payload["value"].as_array().unwrap().as_slice());
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some(expected_message_id.as_str())
        );
        assert_eq!(
            serde_json::from_slice::<Value>(messages[0].payload.as_ref()).unwrap(),
            expected_payload
        );
    }

    #[tokio::test]
    async fn routing_fields_are_not_required_for_collection_publish() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let mut notification = notification_value();
        notification.as_object_mut().unwrap().remove("changeType");
        notification.as_object_mut().unwrap().remove("resourceData");
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-graph.change_notification_collection");
    }

    #[tokio::test]
    async fn validation_tokens_are_preserved_on_published_collection() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "value": [notification_value()],
            "validationTokens": ["jwt-1", "jwt-2"]
        }))
        .unwrap();

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        let payload = serde_json::from_slice::<Value>(messages[0].payload.as_ref()).unwrap();
        assert_eq!(
            payload.get("validationTokens"),
            Some(&serde_json::json!(["jwt-1", "jwt-2"]))
        );
    }

    #[tokio::test]
    async fn publishes_multi_notification_collection_as_one_message() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let second = serde_json::json!({
            "id": "notification-2",
            "subscriptionId": "subscription-1",
            "clientState": TEST_CLIENT_STATE,
            "changeType": "updated",
            "resource": "users('user-1')",
            "resourceData": {
                "id": "user-1",
                "@odata.type": "#Microsoft.Graph.user"
            }
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "value": [notification_value(), second]
        }))
        .unwrap();

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-graph.change_notification_collection");
        let payload = serde_json::from_slice::<Value>(messages[0].payload.as_ref()).unwrap();
        assert_eq!(payload["value"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn mismatched_client_state_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let mut notification = notification_value();
        notification["clientState"] = Value::String("wrong".to_string());
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn missing_client_state_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let mut notification = notification_value();
        notification.as_object_mut().unwrap().remove("clientState");
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_json_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let response = app
            .oneshot(webhook_request("/webhook", Body::from("not-json")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn empty_collection_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({ "value": [] })).unwrap();

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = collection_body(notification_value());

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_no_publishes(&publisher);
    }

    #[test]
    fn validation_token_from_query_ignores_missing_or_empty_tokens() {
        assert_eq!(validation_token_from_query(None), None);
        assert_eq!(validation_token_from_query(Some("ignored=value")), None);
        assert_eq!(validation_token_from_query(Some("validationToken=")), None);
        assert_eq!(
            validation_token_from_query(Some("validationToken=&validationToken=hello%20world")),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn change_notification_collection_message_id_is_stable_for_notification_identity() {
        let first = notification_value();
        let mut second = notification_value();
        second["tenantId"] = Value::String("tenant-changed".to_string());

        assert_eq!(
            change_notification_collection_message_id(&[first]),
            change_notification_collection_message_id(&[second])
        );
    }

    #[test]
    fn change_notification_collection_message_id_changes_for_different_notification_ids() {
        let first = notification_value();
        let mut second = notification_value();
        second["id"] = Value::String("notification-2".to_string());

        assert_ne!(
            change_notification_collection_message_id(&[first]),
            change_notification_collection_message_id(&[second])
        );
    }

    #[test]
    fn change_notification_collection_message_id_uses_payload_when_notification_id_is_missing() {
        let mut first = notification_value();
        first.as_object_mut().unwrap().remove("id");
        let mut second = first.clone();
        second["tenantId"] = Value::String("tenant-changed".to_string());

        assert_ne!(
            change_notification_collection_message_id(&[first]),
            change_notification_collection_message_id(&[second])
        );
    }
}
