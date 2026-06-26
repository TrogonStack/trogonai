use super::config::TelegramSourceConfig;
use super::config::TelegramWebhookSecret;
use super::constants::{
    HEADER_SECRET_TOKEN, HTTP_BODY_SIZE_MAX, NATS_HEADER_UPDATE_ID, NATS_HEADER_UPDATE_TYPE, UPDATE_TYPES,
};
use super::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{info, instrument, warn};
use trogon_semconv::span::TELEGRAM_WEBHOOK;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Telegram update to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("telegram");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: TelegramWebhookSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &TelegramSourceConfig) -> Result<(), C::Error> {
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
    config: &TelegramSourceConfig,
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

/// Extracts the update type by finding the first known field present in the JSON.
///
/// Telegram `Update` objects contain `update_id` plus exactly one optional field
/// that identifies the update type (e.g. `message`, `callback_query`).
fn extract_update_type(value: &serde_json::Value) -> Option<&'static str> {
    let obj = value.as_object()?;
    UPDATE_TYPES.iter().find(|&&t| obj.contains_key(t)).copied()
}

#[instrument(
    name = TELEGRAM_WEBHOOK,
    skip_all,
    fields(
        update_type = tracing::field::Empty,
        update_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let token = headers.get(HEADER_SECRET_TOKEN).and_then(|v| v.to_str().ok());

    if let Err(e) = signature::verify(state.webhook_secret.as_str(), token) {
        warn!(reason = %e, "Telegram webhook secret validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(reason = %e, "Invalid JSON in Telegram webhook body");
            return StatusCode::BAD_REQUEST;
        }
    };

    let update_id = match parsed["update_id"].as_i64() {
        Some(id) => id.to_string(),
        None => {
            warn!("Telegram update missing required update_id field");
            return StatusCode::BAD_REQUEST;
        }
    };

    let update_type = extract_update_type(&parsed).unwrap_or("unroutable");

    let subject = format!("{}.{}", state.subject_prefix, update_type);

    let span = tracing::Span::current();
    span.record(trogon_semconv::attribute::UPDATE_TYPE, update_type);
    span.record(trogon_semconv::attribute::UPDATE_ID, &update_id);
    span.record(trogon_semconv::attribute::SUBJECT, &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, update_id.as_str());
    nats_headers.insert(NATS_HEADER_UPDATE_TYPE, update_type);
    nats_headers.insert(NATS_HEADER_UPDATE_ID, update_id.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests;
