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
use trogon_semconv::span::MICROSOFT_GRAPH_WEBHOOK;
use trogon_std::NonZeroDuration;

use super::config::MicrosoftGraphConfig;
use super::constants::HTTP_BODY_SIZE_MAX;

#[derive(Deserialize)]
struct ChangeNotificationCollection {
    value: Vec<Value>,
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    client_state: super::client_state::MicrosoftGraphClientState,
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
    name = MICROSOFT_GRAPH_WEBHOOK,
    skip_all,
    fields(
        notification_count = tracing::field::Empty,
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
            Some(client_state) if state.client_state.matches(client_state) => {}
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

    let subject = format!("{}.change_notification_collection", state.subject_prefix);
    let span = tracing::Span::current();
    span.record(trogon_semconv::attribute::NOTIFICATION_COUNT, collection.value.len());
    span.record(trogon_semconv::attribute::SUBJECT, &subject);

    let message_id = change_notification_collection_message_id(&collection.value);
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(async_nats::header::NATS_MESSAGE_ID, message_id.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, headers, body, state.nats_ack_timeout.into())
        .await;

    if outcome.is_ok() {
        info!("Published Microsoft Graph notification collection to NATS");
        StatusCode::ACCEPTED
    } else {
        outcome.log_on_error("microsoft-graph");
        StatusCode::INTERNAL_SERVER_ERROR
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
            hasher.update(notification.to_string().as_bytes());
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
mod tests;
