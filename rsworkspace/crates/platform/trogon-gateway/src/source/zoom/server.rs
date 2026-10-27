use std::fmt;
use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    extract::DefaultBodyLimit,
    extract::State,
    http::HeaderMap,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_semconv::span::ZOOM_WEBHOOK;
use trogon_std::time::EpochClock;
use trogon_std::{NonZeroDuration, SystemClock};

use super::ZoomConfig;
use super::ZoomEventType;
use super::ZoomSecretToken;
use super::constants::{
    EVENT_URL_VALIDATION, HEADER_SIGNATURE, HEADER_TIMESTAMP, HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT_TYPE,
    NATS_HEADER_REJECT_REASON, NATS_HEADER_REQUEST_TIMESTAMP,
};
use super::signature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingEventType,
    InvalidEventType,
    MissingPlainToken,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingEventType => "missing_event_type",
            Self::InvalidEventType => "invalid_event_type",
            Self::MissingPlainToken => "missing_plain_token",
        }
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Zoom event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("zoom");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    timestamp: &str,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(NATS_HEADER_REQUEST_TIMESTAMP, timestamp);
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;
    if outcome.is_ok() {
        StatusCode::OK
    } else {
        outcome.log_on_error("zoom.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock> {
    publisher: ClaimCheckPublisher<P, S>,
    clock: C,
    secret_token: ZoomSecretToken,
    subject_prefix: NatsToken,
    timestamp_tolerance: NonZeroDuration,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &ZoomConfig) -> Result<(), C::Error> {
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
    config: &ZoomConfig,
) -> Router {
    router_with_clock(publisher, config, SystemClock)
}

fn router_with_clock<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &ZoomConfig,
    clock: C,
) -> Router {
    let state = AppState {
        publisher,
        clock,
        secret_token: config.secret_token.clone(),
        subject_prefix: config.subject_prefix.clone(),
        timestamp_tolerance: config.timestamp_tolerance,
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S, C>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn url_validation_response(secret_token: &ZoomSecretToken, parsed: &serde_json::Value) -> Option<Response> {
    let plain_token = parsed.pointer("/payload/plainToken").and_then(|value| value.as_str())?;
    match signature::encrypt_plain_token(secret_token.as_str(), plain_token) {
        Ok(encrypted_token) => Some(
            Json(serde_json::json!({
                "plainToken": plain_token,
                "encryptedToken": encrypted_token,
            }))
            .into_response(),
        ),
        Err(err) => {
            warn!(error = %err, "Failed to answer Zoom URL validation");
            Some(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

#[instrument(
    name = ZOOM_WEBHOOK,
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    State(state): State<AppState<P, S, C>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(timestamp) = header_str(&headers, HEADER_TIMESTAMP) else {
        warn!("Missing x-zm-request-timestamp header");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Ok(timestamp_secs) = timestamp.parse::<u64>() else {
        warn!("Invalid x-zm-request-timestamp header");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let now = state
        .clock
        .system_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let drift = now.abs_diff(timestamp_secs);
    if drift > Duration::from(state.timestamp_tolerance).as_secs() {
        warn!(drift_secs = drift, "Zoom request timestamp outside tolerance");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let Some(signature_header) = header_str(&headers, HEADER_SIGNATURE) else {
        warn!("Missing x-zm-signature header");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if let Err(err) = signature::verify(state.secret_token.as_str(), timestamp, &body, signature_header) {
        warn!(error = %err, "Zoom signature validation failed");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(err) => {
            warn!(error = %err, "Failed to parse Zoom webhook body as JSON");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                timestamp,
                RejectReason::InvalidJson,
                body,
                state.nats_ack_timeout,
            )
            .await
            .into_response();
        }
    };

    let Some(raw_event_type) = parsed.get("event").and_then(|value| value.as_str()) else {
        warn!("Missing event in Zoom webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            timestamp,
            RejectReason::MissingEventType,
            body,
            state.nats_ack_timeout,
        )
        .await
        .into_response();
    };

    if raw_event_type == EVENT_URL_VALIDATION {
        info!("Responding to Zoom URL validation");
        return match url_validation_response(&state.secret_token, &parsed) {
            Some(response) => response,
            None => {
                warn!("Missing plainToken in Zoom URL validation payload");
                publish_unroutable(
                    &state.publisher,
                    &state.subject_prefix,
                    timestamp,
                    RejectReason::MissingPlainToken,
                    body,
                    state.nats_ack_timeout,
                )
                .await
                .into_response()
            }
        };
    }

    let event_type = match ZoomEventType::new(raw_event_type) {
        Ok(event_type) => event_type,
        Err(err) => {
            warn!(error = %err, "Invalid event in Zoom webhook payload");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                timestamp,
                RejectReason::InvalidEventType,
                body,
                state.nats_ack_timeout,
            )
            .await
            .into_response();
        }
    };

    let subject = format!("{}.{}", state.subject_prefix, event_type);
    let span = tracing::Span::current();
    span.record(trogon_semconv::attribute::EVENT_TYPE, event_type.as_str());
    span.record(trogon_semconv::attribute::SUBJECT, &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
    nats_headers.insert(NATS_HEADER_REQUEST_TIMESTAMP, timestamp);

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome).into_response()
}

#[cfg(test)]
mod tests;
