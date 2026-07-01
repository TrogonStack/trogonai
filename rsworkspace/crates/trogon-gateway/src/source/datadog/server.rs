use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_semconv::span::DATADOG_WEBHOOK;
use trogon_std::NonZeroDuration;

use super::DatadogConfig;
use super::DatadogEventType;
use super::DatadogWebhookToken;
use super::constants::{
    HEADER_WEBHOOK_TOKEN, HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT_ID, NATS_HEADER_EVENT_TYPE, NATS_HEADER_REJECT_REASON,
};
use super::signature;

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

fn header_value<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn body_event_id(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Reads the `timestamp` field (POSIX epoch seconds, e.g. Datadog's
/// `$DATE_EPOCH`) from the payload. Datadog does not send a timestamp header, so
/// the operator templates it into the body. Accepts a JSON number or a numeric
/// string.
fn body_timestamp_secs(payload: &serde_json::Value) -> Option<u64> {
    let value = payload.get("timestamp")?;
    if let Some(secs) = value.as_u64() {
        return Some(secs);
    }
    if let Some(secs) = value.as_f64().filter(|secs| *secs >= 0.0) {
        return Some(secs as u64);
    }
    value.as_str().and_then(|secs| secs.trim().parse::<u64>().ok())
}

fn timestamp_is_fresh(timestamp_secs: u64, tolerance: NonZeroDuration) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.abs_diff(timestamp_secs) <= Duration::from(tolerance).as_secs()
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Datadog event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("datadog");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    event_id: Option<&str>,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());
    if let Some(event_id) = event_id {
        headers.insert(async_nats::header::NATS_MESSAGE_ID, event_id);
        headers.insert(NATS_HEADER_EVENT_ID, event_id);
    }

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;
    if outcome.is_ok() {
        StatusCode::OK
    } else {
        outcome.log_on_error("datadog.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_token: DatadogWebhookToken,
    subject_prefix: NatsToken,
    timestamp_tolerance: Option<NonZeroDuration>,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &DatadogConfig) -> Result<(), C::Error> {
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
    config: &DatadogConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_token: config.webhook_token.clone(),
        subject_prefix: config.subject_prefix.clone(),
        timestamp_tolerance: config.timestamp_tolerance,
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[instrument(
    name = DATADOG_WEBHOOK,
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        event_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let token = header_value(&headers, HEADER_WEBHOOK_TOKEN);
    if let Err(error) = signature::verify(state.webhook_token.as_str(), token) {
        warn!(reason = %error, "Datadog webhook token validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            warn!(error = %error, "Failed to parse Datadog webhook body as JSON");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                None,
                RejectReason::InvalidJson,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    if let Some(tolerance) = state.timestamp_tolerance {
        let Some(timestamp_secs) = body_timestamp_secs(&payload) else {
            warn!("Missing or invalid timestamp in Datadog webhook payload while tolerance is enabled");
            return StatusCode::BAD_REQUEST;
        };
        if !timestamp_is_fresh(timestamp_secs, tolerance) {
            warn!(timestamp_secs, "Datadog webhook timestamp outside tolerance");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let event_id = body_event_id(&payload).map(str::to_owned);

    let Some(raw_event_type) = payload.get("event_type").and_then(serde_json::Value::as_str) else {
        warn!("Missing event_type in Datadog webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            event_id.as_deref(),
            RejectReason::MissingEventType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let event_type = match DatadogEventType::new(raw_event_type) {
        Ok(event_type) => event_type,
        Err(error) => {
            warn!(error = %error, "Invalid event_type in Datadog webhook payload");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                event_id.as_deref(),
                RejectReason::InvalidEventType,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    let subject = format!("{}.{}", state.subject_prefix, event_type);
    let span = tracing::Span::current();
    span.record(trogon_semconv::attribute::EVENT_TYPE, event_type.as_str());
    if let Some(event_id) = event_id.as_deref() {
        span.record(trogon_semconv::attribute::EVENT_ID, event_id);
    }
    span.record(trogon_semconv::attribute::SUBJECT, &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
    if let Some(event_id) = event_id.as_deref() {
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, event_id);
        nats_headers.insert(NATS_HEADER_EVENT_ID, event_id);
    }

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
