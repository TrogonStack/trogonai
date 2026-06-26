use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use super::config::SentryConfig;
use super::constants::{
    HEADER_REQUEST_ID, HEADER_RESOURCE, HEADER_SIGNATURE, HEADER_TIMESTAMP, HTTP_BODY_SIZE_MAX, NATS_HEADER_ACTION,
    NATS_HEADER_REQUEST_ID, NATS_HEADER_RESOURCE, NATS_HEADER_TIMESTAMP,
};
use super::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use serde::Deserialize;
use tracing::{info, instrument, warn};
use trogon_semconv::span::SENTRY_WEBHOOK;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

#[derive(Deserialize)]
struct WebhookEnvelope {
    action: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SentryResource {
    Installation,
    EventAlert,
    Issue,
    MetricAlert,
    Error,
    Comment,
    Seer,
}

impl SentryResource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Installation => "installation",
            Self::EventAlert => "event_alert",
            Self::Issue => "issue",
            Self::MetricAlert => "metric_alert",
            Self::Error => "error",
            Self::Comment => "comment",
            Self::Seer => "seer",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unsupported Sentry webhook resource")]
struct InvalidSentryResource;

impl FromStr for SentryResource {
    type Err = InvalidSentryResource;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "installation" => Ok(Self::Installation),
            "event_alert" => Ok(Self::EventAlert),
            "issue" => Ok(Self::Issue),
            "metric_alert" => Ok(Self::MetricAlert),
            "error" => Ok(Self::Error),
            "comment" => Ok(Self::Comment),
            "seer" => Ok(Self::Seer),
            _ => Err(InvalidSentryResource),
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

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Sentry event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("sentry");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    client_secret: super::sentry_client_secret::SentryClientSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &SentryConfig) -> Result<(), C::Error> {
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
    config: &SentryConfig,
) -> Router {
    let state = AppState {
        publisher,
        client_secret: config.client_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[instrument(
    name = SENTRY_WEBHOOK,
    skip_all,
    fields(
        resource = tracing::field::Empty,
        action = tracing::field::Empty,
        request_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(signature) = header_value(&headers, HEADER_SIGNATURE) else {
        warn!("Missing Sentry-Hook-Signature header");
        return StatusCode::UNAUTHORIZED;
    };

    if let Err(error) = signature::verify(state.client_secret.as_str(), &body, signature) {
        warn!(reason = %error, "Sentry webhook signature validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let Some(resource) = header_value(&headers, HEADER_RESOURCE) else {
        warn!("Missing Sentry-Hook-Resource header");
        return StatusCode::BAD_REQUEST;
    };

    let Some(timestamp) = header_value(&headers, HEADER_TIMESTAMP).map(str::to_owned) else {
        warn!("Missing Sentry-Hook-Timestamp header");
        return StatusCode::BAD_REQUEST;
    };
    let Some(request_id) = header_value(&headers, HEADER_REQUEST_ID).map(str::to_owned) else {
        warn!("Missing Request-ID header");
        return StatusCode::BAD_REQUEST;
    };

    let payload = match serde_json::from_slice::<WebhookEnvelope>(&body) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(error = %error, "Failed to parse Sentry webhook payload");
            return StatusCode::BAD_REQUEST;
        }
    };

    let resource = match resource.parse::<SentryResource>() {
        Ok(resource) => resource,
        Err(error) => {
            warn!(reason = %error, resource, "Unsupported Sentry webhook resource");
            return StatusCode::BAD_REQUEST;
        }
    };
    let action_token = match NatsToken::new(payload.action.as_str()) {
        Ok(token) => token,
        Err(error) => {
            warn!(reason = ?error, action = %payload.action, "Invalid Sentry action token");
            return StatusCode::BAD_REQUEST;
        }
    };

    let subject = format!("{}.{}.{}", state.subject_prefix, resource.as_str(), action_token);
    let span = tracing::Span::current();
    span.record(trogon_semconv::attribute::RESOURCE, resource.as_str());
    span.record(trogon_semconv::attribute::ACTION, &payload.action);
    span.record(trogon_semconv::attribute::REQUEST_ID, &request_id);
    span.record(trogon_semconv::attribute::SUBJECT, &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(NATS_HEADER_RESOURCE, resource.as_str());
    nats_headers.insert(NATS_HEADER_ACTION, payload.action.as_str());
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, request_id.as_str());
    nats_headers.insert(NATS_HEADER_REQUEST_ID, request_id.as_str());
    nats_headers.insert(NATS_HEADER_TIMESTAMP, timestamp.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
