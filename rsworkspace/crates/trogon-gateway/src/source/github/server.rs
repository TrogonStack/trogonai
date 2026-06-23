use std::fmt;
use std::time::Duration;

use super::config::GitHubWebhookSecret;
use super::config::GithubConfig;
use super::constants::{
    HEADER_DELIVERY, HEADER_EVENT, HEADER_SIGNATURE, HTTP_BODY_SIZE_MAX, NATS_HEADER_DELIVERY, NATS_HEADER_EVENT,
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

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published GitHub event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("github");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: GitHubWebhookSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &GithubConfig) -> Result<(), C::Error> {
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
    config: &GithubConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
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
    name = "github.webhook",
    skip_all,
    fields(
        event = tracing::field::Empty,
        delivery = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = if let Some(v) = headers.get(HEADER_SIGNATURE) {
        v.to_str().ok()
    } else {
        None
    };

    match sig {
        Some(sig) => {
            if let Err(e) = signature::verify(state.webhook_secret.as_str(), &body, sig) {
                warn!(reason = %e, "GitHub webhook signature validation failed");
                return StatusCode::UNAUTHORIZED;
            }
        }
        None => {
            warn!("Missing X-Hub-Signature-256 header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let Some(event) = headers
        .get(HEADER_EVENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        warn!("Missing X-GitHub-Event header");
        return StatusCode::BAD_REQUEST;
    };

    let delivery = headers
        .get(HEADER_DELIVERY)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}", state.subject_prefix, event);

    let span = tracing::Span::current();
    span.record("event", &event);
    span.record("delivery", &delivery);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, delivery.as_str());
    nats_headers.insert(NATS_HEADER_EVENT, event.as_str());
    nats_headers.insert(NATS_HEADER_DELIVERY, delivery.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
