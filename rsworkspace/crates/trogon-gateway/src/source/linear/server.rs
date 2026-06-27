use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::config::LinearConfig;
use super::config::LinearWebhookSecret;
use super::constants::{HTTP_BODY_SIZE_MAX, NATS_HEADER_REJECT_REASON};
use super::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_semconv::span::LINEAR_WEBHOOK;
use trogon_std::NonZeroDuration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingWebhookTimestamp,
    StaleWebhookTimestamp,
    MissingType,
    InvalidType,
    MissingAction,
    InvalidAction,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingWebhookTimestamp => "missing_webhook_timestamp",
            Self::StaleWebhookTimestamp => "stale_webhook_timestamp",
            Self::MissingType => "missing_type",
            Self::InvalidType => "invalid_type",
            Self::MissingAction => "missing_action",
            Self::InvalidAction => "invalid_action",
        }
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Linear event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("linear");
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

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;
    if outcome.is_ok() {
        StatusCode::BAD_REQUEST
    } else {
        outcome.log_on_error("linear.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: LinearWebhookSecret,
    subject_prefix: NatsToken,
    timestamp_tolerance: Option<NonZeroDuration>,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &LinearConfig) -> Result<(), C::Error> {
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
    config: &LinearConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        timestamp_tolerance: config.timestamp_tolerance,
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = StatusCode> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

#[instrument(
    name = LINEAR_WEBHOOK,
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        action = tracing::field::Empty,
        webhook_id = tracing::field::Empty,
        subject = tracing::field::Empty,
        messaging.message.body.size = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = headers.get("linear-signature").and_then(|v| v.to_str().ok());

    match sig {
        Some(sig) if signature::verify(state.webhook_secret.as_str(), &body, sig) => {}
        Some(_) => {
            warn!("Invalid Linear webhook signature");
            return StatusCode::UNAUTHORIZED;
        }
        None => {
            warn!("Missing linear-signature header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse Linear webhook body as JSON");
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

    if let Some(tolerance) = state.timestamp_tolerance {
        let Some(ts_ms) = parsed.get("webhookTimestamp").and_then(|v| v.as_u64()) else {
            warn!("Missing or malformed 'webhookTimestamp' field");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::MissingWebhookTimestamp,
                body,
                state.nats_ack_timeout,
            )
            .await;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age_ms = now_ms.saturating_sub(ts_ms);
        if age_ms > Duration::from(tolerance).as_millis() as u64 {
            warn!(
                age_ms,
                tolerance_ms = Duration::from(tolerance).as_millis() as u64,
                "Stale webhookTimestamp — potential replay attack"
            );
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::StaleWebhookTimestamp,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    }

    let Some(raw_type) = parsed.get("type").and_then(|v| v.as_str()) else {
        warn!("Missing 'type' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };
    let Ok(event_type) = NatsToken::new(raw_type) else {
        warn!("Invalid 'type' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let Some(raw_action) = parsed.get("action").and_then(|v| v.as_str()) else {
        warn!("Missing 'action' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingAction,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };
    let Ok(action) = NatsToken::new(raw_action) else {
        warn!("Invalid 'action' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidAction,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let webhook_id = parsed
        .get("webhookId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}.{}", state.subject_prefix, event_type, action);

    let span = tracing::Span::current();
    span.record(trogon_semconv::attribute::EVENT_TYPE, event_type.as_str());
    span.record(trogon_semconv::attribute::ACTION, action.as_str());
    span.record(trogon_semconv::attribute::WEBHOOK_ID, &webhook_id);
    span.record(trogon_semconv::attribute::SUBJECT, &subject);
    span.record(trogon_semconv::attribute::MESSAGING_MESSAGE_BODY_SIZE, body.len());

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("Nats-Msg-Id", webhook_id.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
