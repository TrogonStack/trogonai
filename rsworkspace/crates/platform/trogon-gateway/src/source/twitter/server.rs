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
use trogon_semconv::span::TWITTER_CRC;
use trogon_semconv::span::TWITTER_WEBHOOK;
use trogon_std::NonZeroDuration;

use super::config::{TwitterConfig, TwitterConsumerSecret};
use super::constants::{
    HEADER_SIGNATURE, HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT_TYPE, NATS_HEADER_PAYLOAD_KIND, NATS_HEADER_REJECT_REASON,
};
use super::signature;

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

pub async fn provision<C: JetStreamContext>(js: &C, config: &TwitterConfig) -> Result<(), C::Error> {
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
        .route("/webhook", get(handle_crc::<P, S>).post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[instrument(name = TWITTER_CRC, skip_all)]
async fn handle_crc<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    Query(query): Query<CrcQuery>,
) -> Result<Json<CrcResponse>, StatusCode> {
    if query.crc_token.is_empty() {
        warn!("Empty crc_token query parameter");
        return Err(StatusCode::BAD_REQUEST);
    }

    let response_token = signature::crc_response_token(state.consumer_secret.as_str(), &query.crc_token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CrcResponse { response_token }))
}

#[instrument(
    name = TWITTER_WEBHOOK,
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
    let Some(signature_header) = headers.get(HEADER_SIGNATURE).and_then(|value| value.to_str().ok()) else {
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
            warn!(reason = reason.as_str(), "Unable to classify Twitter/X webhook payload");
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
    span.record(trogon_semconv::attribute::EVENT_TYPE, event_type.as_str());
    span.record(trogon_semconv::attribute::PAYLOAD_KIND, payload_kind.as_str());
    span.record(trogon_semconv::attribute::SUBJECT, &subject);

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

fn resolve_event_type(payload: &serde_json::Value) -> Result<(DottedNatsToken, PayloadKind), RejectReason> {
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
        return DottedNatsToken::new("filtered_stream")
            .map(|event_type| (event_type, PayloadKind::FilteredStream))
            .map_err(|_| RejectReason::InvalidEventType);
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

            payload.get(event_type).and_then(|event_value| match event_value {
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
mod tests;
