use std::fmt;
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
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

use crate::config::MicrosoftTeamsConfig;
use crate::constants::{
    HTTP_BODY_SIZE_MAX, NATS_HEADER_CHANGE_TYPE, NATS_HEADER_NOTIFICATION_ID, NATS_HEADER_REJECT_REASON,
    NATS_HEADER_RESOURCE_TYPE, NATS_HEADER_SUBSCRIPTION_ID,
};

#[derive(Deserialize)]
struct ChangeNotificationCollection {
    value: Vec<Value>,
    #[serde(default, rename = "validationTokens")]
    validation_tokens: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidChange,
    MissingChange,
    MissingResource,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidChange => "invalid_change_type",
            Self::MissingChange => "missing_change_type",
            Self::MissingResource => "missing_resource_type",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct NotificationMetadata {
    id: Option<String>,
    subscription_id: Option<String>,
    client_state: Option<String>,
    change_type: Option<String>,
    resource_type: Option<&'static str>,
}

impl NotificationMetadata {
    fn from_value(value: &Value) -> Self {
        let resource_data = value.get("resourceData").unwrap_or(&Value::Null);
        let resource_type = resource_data
            .get("@odata.type")
            .and_then(Value::as_str)
            .and_then(graph_resource_kind);

        Self {
            id: value.get("id").and_then(Value::as_str).map(str::to_owned),
            subscription_id: value.get("subscriptionId").and_then(Value::as_str).map(str::to_owned),
            client_state: value.get("clientState").and_then(Value::as_str).map(str::to_owned),
            change_type: value.get("changeType").and_then(Value::as_str).map(str::to_owned),
            resource_type,
        }
    }

    fn apply_headers(&self, headers: &mut async_nats::HeaderMap) {
        if let Some(ref id) = self.id {
            headers.insert(async_nats::header::NATS_MESSAGE_ID, id.as_str());
            headers.insert(NATS_HEADER_NOTIFICATION_ID, id.as_str());
        }
        if let Some(ref subscription_id) = self.subscription_id {
            headers.insert(NATS_HEADER_SUBSCRIPTION_ID, subscription_id.as_str());
        }
        if let Some(resource_type) = self.resource_type {
            headers.insert(NATS_HEADER_RESOURCE_TYPE, resource_type);
        }
        if let Some(ref change_type) = self.change_type {
            headers.insert(NATS_HEADER_CHANGE_TYPE, change_type.as_str());
        }
    }

    fn resource_kind(&self) -> Option<&'static str> {
        self.resource_type
    }

    fn change_type_token(&self) -> Result<NatsToken, RejectReason> {
        let change_type = self.change_type.as_deref().ok_or(RejectReason::MissingChange)?;
        NatsToken::new(change_type).map_err(|_| RejectReason::InvalidChange)
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    client_state: crate::client_state::MicrosoftTeamsClientState,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &MicrosoftTeamsConfig) -> Result<(), C::Error> {
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
    config: &MicrosoftTeamsConfig,
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

async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    if let Some(validation_token) = validation_token_from_query(query.as_deref()) {
        info!("Responding to Microsoft Graph notification URL validation");
        return (StatusCode::OK, [(CONTENT_TYPE, "text/plain")], validation_token).into_response();
    }

    handle_notification_collection(state, body).await.into_response()
}

#[instrument(
    name = "microsoft_teams.webhook",
    skip_all,
    fields(
        notification_count = tracing::field::Empty,
        subscription_id = tracing::field::Empty,
        resource_type = tracing::field::Empty,
        change_type = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_notification_collection<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    body: Bytes,
) -> StatusCode {
    let collection = match serde_json::from_slice::<ChangeNotificationCollection>(&body) {
        Ok(collection) => collection,
        Err(error) => {
            warn!(error = %error, "Failed to parse Microsoft Teams webhook body as JSON");
            return StatusCode::BAD_REQUEST;
        }
    };

    if collection.value.is_empty() {
        warn!("Microsoft Teams webhook body contained no notifications");
        return StatusCode::BAD_REQUEST;
    }

    let validation_tokens = collection.validation_tokens.filter(|tokens| !tokens.is_empty());
    let notifications: Vec<(Value, NotificationMetadata)> = collection
        .value
        .into_iter()
        .map(|mut notification| {
            if let Some(ref tokens) = validation_tokens {
                attach_validation_tokens(&mut notification, tokens);
            }
            let metadata = NotificationMetadata::from_value(&notification);
            (notification, metadata)
        })
        .collect();

    for (_, metadata) in &notifications {
        match metadata.client_state.as_deref() {
            Some(client_state) if state.client_state.matches(client_state) => {}
            Some(_) => {
                warn!(
                    subscription_id = metadata.subscription_id.as_deref().unwrap_or("unknown"),
                    "Microsoft Teams clientState validation failed"
                );
                return StatusCode::UNAUTHORIZED;
            }
            None => {
                warn!(
                    subscription_id = metadata.subscription_id.as_deref().unwrap_or("unknown"),
                    "Microsoft Teams notification missing clientState"
                );
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    tracing::Span::current().record("notification_count", notifications.len());

    for (notification, metadata) in notifications {
        let status = publish_notification(&state, notification, &metadata).await;
        if status.is_server_error() {
            return status;
        }
    }

    StatusCode::ACCEPTED
}

async fn publish_notification<P: JetStreamPublisher, S: ObjectStorePut>(
    state: &AppState<P, S>,
    notification: Value,
    metadata: &NotificationMetadata,
) -> StatusCode {
    let payload = notification_payload(&notification);

    let Some(resource_kind) = metadata.resource_kind() else {
        warn!("Microsoft Teams notification is missing a routable resource type");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            metadata,
            RejectReason::MissingResource,
            payload,
            state.nats_ack_timeout,
        )
        .await;
    };

    let change_type = match metadata.change_type_token() {
        Ok(change_type) => change_type,
        Err(reason) => {
            warn!("Microsoft Teams notification is missing a routable change type");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                metadata,
                reason,
                payload,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    let subject = format!("{}.{}.{}", state.subject_prefix, resource_kind, change_type);
    let span = tracing::Span::current();
    span.record(
        "subscription_id",
        metadata.subscription_id.as_deref().unwrap_or("unknown"),
    );
    span.record("resource_type", resource_kind);
    span.record("change_type", change_type.as_str());
    span.record("subject", &subject);

    let mut headers = async_nats::HeaderMap::new();
    metadata.apply_headers(&mut headers);

    let outcome = state
        .publisher
        .publish_event(subject, headers, payload, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    metadata: &NotificationMetadata,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    metadata.apply_headers(&mut headers);
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;

    if outcome.is_ok() {
        StatusCode::ACCEPTED
    } else {
        outcome.log_on_error("microsoft-teams.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Microsoft Teams event to NATS");
        StatusCode::ACCEPTED
    } else {
        outcome.log_on_error("microsoft-teams");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn notification_payload(notification: &Value) -> Bytes {
    Bytes::from(serde_json::to_vec(notification).expect("serde_json::Value serialization should not fail"))
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

fn attach_validation_tokens(notification: &mut Value, tokens: &[String]) {
    let Some(notification) = notification.as_object_mut() else {
        return;
    };

    notification.insert(
        "validationTokens".to_string(),
        Value::Array(tokens.iter().cloned().map(Value::String).collect()),
    );
}

fn graph_resource_kind(raw: &str) -> Option<&'static str> {
    const GRAPH_NAMESPACE: &str = "microsoft.graph.";

    let raw = raw.trim().trim_start_matches('#');
    let graph_type = raw
        .get(..GRAPH_NAMESPACE.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(GRAPH_NAMESPACE))
        .and_then(|_| raw.get(GRAPH_NAMESPACE.len()..))
        .unwrap_or(raw);

    match graph_type.to_ascii_lowercase().as_str() {
        "aaduserconversationmember" => Some("aad_user_conversation_member"),
        "callrecording" => Some("call_recording"),
        "calltranscript" => Some("call_transcript"),
        "channel" => Some("channel"),
        "chat" => Some("chat"),
        "chatmessage" => Some("chat_message"),
        "conversationmember" => Some("conversation_member"),
        "onlinemeeting" => Some("online_meeting"),
        "presence" => Some("presence"),
        "team" | "teams" => Some("team"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_state::MicrosoftTeamsClientState;
    use crate::constants::{
        NATS_HEADER_CHANGE_TYPE, NATS_HEADER_NOTIFICATION_ID, NATS_HEADER_REJECT_REASON, NATS_HEADER_RESOURCE_TYPE,
        NATS_HEADER_SUBSCRIPTION_ID,
    };
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
    };

    const TEST_CLIENT_STATE: &str = "secret-client-state";

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

    fn test_config() -> MicrosoftTeamsConfig {
        MicrosoftTeamsConfig {
            client_state: MicrosoftTeamsClientState::new(TEST_CLIENT_STATE).unwrap(),
            subject_prefix: NatsToken::new("microsoft-teams").unwrap(),
            stream_name: NatsToken::new("MICROSOFT_TEAMS").unwrap(),
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

    fn webhook_request(uri: &str, body: impl Into<Body>) -> Request<Body> {
        Request::builder().method("POST").uri(uri).body(body.into()).unwrap()
    }

    fn notification_value() -> Value {
        serde_json::json!({
            "id": "notification-1",
            "subscriptionId": "subscription-1",
            "subscriptionExpirationDateTime": "2026-04-28T17:00:00Z",
            "clientState": TEST_CLIENT_STATE,
            "changeType": "created",
            "tenantId": "tenant-1",
            "resource": "teams('team-1')/channels('channel-1')/messages('message-1')",
            "resourceData": {
                "id": "message-1",
                "@odata.type": "#Microsoft.Graph.chatMessage",
                "@odata.id": "teams('team-1')/channels('channel-1')/messages('message-1')"
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
        assert_eq!(streams[0].name, "MICROSOFT_TEAMS");
        assert_eq!(streams[0].subjects, vec!["microsoft-teams.>"]);
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
    async fn valid_notification_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let notification = notification_value();
        let body = collection_body(notification.clone());

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-teams.chat_message.created");
        assert_eq!(
            serde_json::from_slice::<Value>(messages[0].payload.as_ref()).unwrap(),
            notification
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("notification-1")
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_NOTIFICATION_ID)
                .map(|value| value.as_str()),
            Some("notification-1")
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_SUBSCRIPTION_ID)
                .map(|value| value.as_str()),
            Some("subscription-1")
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_RESOURCE_TYPE)
                .map(|value| value.as_str()),
            Some("chat_message")
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_CHANGE_TYPE)
                .map(|value| value.as_str()),
            Some("created")
        );
    }

    #[tokio::test]
    async fn validation_tokens_are_preserved_on_published_notification() {
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
    async fn publishes_each_notification_in_collection() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let second = serde_json::json!({
            "id": "notification-2",
            "subscriptionId": "subscription-1",
            "clientState": TEST_CLIENT_STATE,
            "changeType": "updated",
            "resource": "teams('team-1')/channels('channel-1')",
            "resourceData": {
                "id": "channel-1",
                "@odata.type": "#Microsoft.Graph.channel"
            }
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "value": [notification_value(), second]
        }))
        .unwrap();

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            publisher.published_subjects(),
            vec![
                "microsoft-teams.chat_message.created",
                "microsoft-teams.channel.updated"
            ]
        );
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
    async fn missing_resource_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let notification = serde_json::json!({
            "id": "notification-1",
            "subscriptionId": "subscription-1",
            "clientState": TEST_CLIENT_STATE,
            "changeType": "created"
        });
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-teams.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some("missing_resource_type")
        );
    }

    #[tokio::test]
    async fn unknown_resource_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let mut notification = notification_value();
        notification["resourceData"]["@odata.type"] = Value::String("#Microsoft.Graph.todoTask".to_string());
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-teams.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some("missing_resource_type")
        );
    }

    #[tokio::test]
    async fn missing_change_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let mut notification = notification_value();
        notification.as_object_mut().unwrap().remove("changeType");
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-teams.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some("missing_change_type")
        );
    }

    #[tokio::test]
    async fn invalid_change_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let mut notification = notification_value();
        notification["changeType"] = Value::String("updated.created".to_string());
        let body = collection_body(notification);

        let response = app.oneshot(webhook_request("/webhook", body)).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "microsoft-teams.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some("invalid_change_type")
        );
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

    #[tokio::test]
    async fn unroutable_publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let notification = serde_json::json!({
            "id": "notification-1",
            "subscriptionId": "subscription-1",
            "clientState": TEST_CLIENT_STATE,
            "changeType": "created"
        });
        let body = collection_body(notification);

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
    fn attach_validation_tokens_skips_non_object_notifications() {
        let mut notification = Value::String("not-an-object".to_string());

        attach_validation_tokens(&mut notification, &["jwt".to_string()]);

        assert_eq!(notification, Value::String("not-an-object".to_string()));
    }

    #[test]
    fn graph_resource_kind_maps_known_resource_types() {
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.aadUserConversationMember"),
            Some("aad_user_conversation_member")
        );
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.callRecording"),
            Some("call_recording")
        );
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.callTranscript"),
            Some("call_transcript")
        );
        assert_eq!(graph_resource_kind("#Microsoft.Graph.channel"), Some("channel"));
        assert_eq!(graph_resource_kind("#Microsoft.Graph.Channel"), Some("channel"));
        assert_eq!(graph_resource_kind("#Microsoft.Graph.chat"), Some("chat"));
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.chatMessage"),
            Some("chat_message")
        );
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.ChatMessage"),
            Some("chat_message")
        );
        assert_eq!(graph_resource_kind("chatMessage"), Some("chat_message"));
        assert_eq!(graph_resource_kind("#Other.Graph.chatMessage"), None);
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.conversationMember"),
            Some("conversation_member")
        );
        assert_eq!(
            graph_resource_kind("#Microsoft.Graph.onlineMeeting"),
            Some("online_meeting")
        );
        assert_eq!(graph_resource_kind("#Microsoft.Graph.presence"), Some("presence"));
        assert_eq!(graph_resource_kind("#Microsoft.Graph.team"), Some("team"));
        assert_eq!(graph_resource_kind("#Microsoft.Graph.Team"), Some("team"));
        assert_eq!(graph_resource_kind("#Microsoft.Graph.Teams"), Some("team"));
        assert_eq!(graph_resource_kind("#Microsoft.Graph.todoTask"), None);
        assert_eq!(graph_resource_kind("Microsoft.Graphé.chatMessage"), None);
        assert_eq!(graph_resource_kind("#Microsoft.Graph."), None);
    }
}
