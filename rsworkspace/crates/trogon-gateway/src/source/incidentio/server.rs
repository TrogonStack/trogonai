use std::fmt;
use std::time::Duration;

use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

use super::IncidentioConfig;
use super::IncidentioEventType;
use super::IncidentioSigningSecret;
use super::constants::{
    HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT_TYPE, NATS_HEADER_REJECT_REASON, NATS_HEADER_WEBHOOK_ID,
    NATS_HEADER_WEBHOOK_TIMESTAMP,
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

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published incident.io event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("incidentio");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    verified: &signature::VerifiedWebhook,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(async_nats::header::NATS_MESSAGE_ID, verified.webhook_id.as_str());
    headers.insert(NATS_HEADER_WEBHOOK_ID, verified.webhook_id.as_str());
    headers.insert(NATS_HEADER_WEBHOOK_TIMESTAMP, verified.webhook_timestamp.as_str());
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;
    if outcome.is_ok() {
        StatusCode::OK
    } else {
        outcome.log_on_error("incidentio.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    signing_secret: IncidentioSigningSecret,
    subject_prefix: NatsToken,
    timestamp_tolerance: NonZeroDuration,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &IncidentioConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.to_string(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        duplicate_window: config.timestamp_tolerance.into(),
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
    config: &IncidentioConfig,
) -> Router {
    let state = AppState {
        publisher,
        signing_secret: config.signing_secret.clone(),
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
    name = "incidentio.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        webhook_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let verified = match signature::verify(&headers, &body, &state.signing_secret, state.timestamp_tolerance) {
        Ok(verified) => verified,
        Err(err) => {
            warn!(error = %err, "incident.io webhook signature validation failed");
            return StatusCode::UNAUTHORIZED;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(err) => {
            warn!(error = %err, "Failed to parse incident.io webhook body as JSON");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                &verified,
                RejectReason::InvalidJson,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    let Some(raw_event_type) = parsed.get("event_type").and_then(|value| value.as_str()) else {
        warn!("Missing event_type in incident.io webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            &verified,
            RejectReason::MissingEventType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let event_type = match IncidentioEventType::new(raw_event_type) {
        Ok(event_type) => event_type,
        Err(err) => {
            warn!(error = %err, "Invalid event_type in incident.io webhook payload");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                &verified,
                RejectReason::InvalidEventType,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    let subject = format!("{}.{}", state.subject_prefix, event_type);
    let span = tracing::Span::current();
    span.record("event_type", event_type.as_str());
    span.record("webhook_id", tracing::field::display(verified.webhook_id.as_str()));
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, verified.webhook_id.as_str());
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
    nats_headers.insert(NATS_HEADER_WEBHOOK_ID, verified.webhook_id.as_str());
    nats_headers.insert(NATS_HEADER_WEBHOOK_TIMESTAMP, verified.webhook_timestamp.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
