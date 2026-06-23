use std::fmt;
use std::time::Duration;

use super::GitLabSigningToken;
use super::config::GitlabConfig;
use super::constants::{
    HEADER_EVENT, HEADER_EVENT_UUID, HEADER_IDEMPOTENCY_KEY, HEADER_INSTANCE, HEADER_WEBHOOK_UUID, HTTP_BODY_SIZE_MAX,
    NATS_HEADER_EVENT, NATS_HEADER_EVENT_UUID, NATS_HEADER_INSTANCE, NATS_HEADER_REJECT_REASON,
    NATS_HEADER_WEBHOOK_UUID,
};
use super::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use std::future::Future;
use std::pin::Pin;
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    MissingEventHeader,
    InvalidEventToken,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingEventHeader => "missing_event_header",
            Self::InvalidEventToken => "invalid_event_token",
        }
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
        // Return 200 so GitLab doesn't count this as a failure and
        // auto-disable the webhook after repeated 4xx responses.
        StatusCode::OK
    } else {
        outcome.log_on_error("gitlab.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published GitLab event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("gitlab");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    signing_token: GitLabSigningToken,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
    timestamp_tolerance: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &GitlabConfig) -> Result<(), C::Error> {
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
    config: &GitlabConfig,
) -> Router {
    let state = AppState {
        publisher,
        signing_token: config.signing_token.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
        timestamp_tolerance: config.timestamp_tolerance,
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
    name = "gitlab.webhook",
    skip_all,
    fields(
        event = tracing::field::Empty,
        webhook_uuid = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if let Err(e) = signature::verify(&headers, &body, &state.signing_token, state.timestamp_tolerance) {
        warn!(reason = %e, "GitLab webhook signature validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let Some(raw_event) = headers.get(HEADER_EVENT).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-GitLab-Event header");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingEventHeader,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let event_token: String = raw_event
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();

    let Ok(event_token) = NatsToken::new(&event_token) else {
        warn!(raw_event, "X-GitLab-Event normalized to invalid NATS token");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidEventToken,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let webhook_uuid = headers.get(HEADER_WEBHOOK_UUID).and_then(|v| v.to_str().ok());
    let event_uuid = headers.get(HEADER_EVENT_UUID).and_then(|v| v.to_str().ok());
    let idempotency_key = headers.get(HEADER_IDEMPOTENCY_KEY).and_then(|v| v.to_str().ok());
    let instance = headers.get(HEADER_INSTANCE).and_then(|v| v.to_str().ok());

    let subject = format!("{}.{}", state.subject_prefix, event_token);

    let span = tracing::Span::current();
    span.record("event", raw_event);
    span.record("webhook_uuid", webhook_uuid.unwrap_or("unknown"));
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_EVENT, raw_event);
    if let Some(id) = idempotency_key {
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, id);
    }
    if let Some(uuid) = webhook_uuid {
        nats_headers.insert(NATS_HEADER_WEBHOOK_UUID, uuid);
    }
    if let Some(euuid) = event_uuid {
        nats_headers.insert(NATS_HEADER_EVENT_UUID, euuid);
    }
    if let Some(inst) = instance {
        nats_headers.insert(NATS_HEADER_INSTANCE, inst);
    }

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
