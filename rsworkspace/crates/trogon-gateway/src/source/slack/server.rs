use std::fmt;
use std::time::Duration;

use super::config::SlackConfig;
use super::config::SlackSigningSecret;
use super::config::SlackWebhookConfig;
use super::constants::{
    CONTENT_TYPE_FORM, HEADER_SIGNATURE, HEADER_TIMESTAMP, HTTP_BODY_SIZE_MAX, NATS_HEADER_EVENT_ID,
    NATS_HEADER_EVENT_TYPE, NATS_HEADER_PAYLOAD_KIND, NATS_HEADER_REJECT_REASON, NATS_HEADER_TEAM_ID,
};
use super::signature;
use trogon_std::NonZeroDuration;
use trogon_std::SystemClock;
use trogon_std::time::EpochClock;

use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use form_urlencoded;
use std::future::Future;
use std::pin::Pin;
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_semconv::span::SLACK_WEBHOOK;

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Slack event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("slack");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
pub struct SlackBridge<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock> {
    bridge: SlackBridge<P, S>,
    clock: C,
    signing_secret: SlackSigningSecret,
    timestamp_max_drift: NonZeroDuration,
}

impl<P: JetStreamPublisher, S: ObjectStorePut> SlackBridge<P, S> {
    pub fn new(publisher: ClaimCheckPublisher<P, S>, config: &SlackConfig) -> Self {
        Self {
            publisher,
            subject_prefix: config.subject_prefix.clone(),
            nats_ack_timeout: config.nats_ack_timeout,
        }
    }

    pub async fn handle_json_body(&self, body: Bytes) -> (StatusCode, String) {
        let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
            warn!("Invalid JSON payload");
            self.publish_unroutable("invalid_json", body).await;
            return (StatusCode::BAD_REQUEST, String::new());
        };

        self.handle_json_payload(&body, &payload).await
    }

    pub async fn handle_socket_interaction(
        &self,
        payload: &serde_json::Value,
    ) -> Result<StatusCode, serde_json::Error> {
        let payload_json = serde_json::to_string(payload)?;
        let status = self
            .handle_interaction(&payload_json, Bytes::from(payload_json.clone()))
            .await;
        Ok(status)
    }

    pub async fn handle_socket_slash_command(
        &self,
        payload: &serde_json::Value,
    ) -> Result<StatusCode, serde_json::Error> {
        let raw_body = Bytes::from(serde_json::to_vec(payload)?);
        Ok(self.handle_slash_command_value(payload, raw_body).await)
    }

    pub async fn publish_socket_unroutable(&self, reason: &str, body: Bytes) {
        self.publish_unroutable(reason, body).await;
    }

    async fn publish_unroutable(&self, reason: &str, body: Bytes) {
        let subject = format!("{}.unroutable", self.subject_prefix);
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(NATS_HEADER_REJECT_REASON, reason);
        headers.insert(NATS_HEADER_PAYLOAD_KIND, "unroutable");

        let outcome = self
            .publisher
            .publish_event(subject, headers, body, self.nats_ack_timeout.into())
            .await;
        outcome.log_on_error("slack.unroutable");
    }

    async fn handle_json_payload(&self, body: &Bytes, payload: &serde_json::Value) -> (StatusCode, String) {
        let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or_default();

        if payload_type == "url_verification" {
            let challenge = payload
                .get("challenge")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            info!("Responding to Slack URL verification challenge");
            return (StatusCode::OK, challenge);
        }

        if payload_type != "event_callback" {
            warn!(payload_type, "Unhandled payload type");
            self.publish_unroutable("unhandled_payload_type", body.clone()).await;
            return (StatusCode::OK, String::new());
        }

        let Some(event_type) = payload
            .get("event")
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str())
        else {
            warn!("Missing event.type in event_callback payload");
            self.publish_unroutable("missing_event_type", body.clone()).await;
            return (StatusCode::BAD_REQUEST, String::new());
        };
        let event_type = event_type.to_owned();

        let Some(event_id) = payload.get("event_id").and_then(|v| v.as_str()) else {
            warn!("Missing event_id in event_callback payload");
            self.publish_unroutable("missing_event_id", body.clone()).await;
            return (StatusCode::BAD_REQUEST, String::new());
        };
        let event_id = event_id.to_owned();

        let team_id = payload
            .get("team_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();

        let subject = format!("{}.event.{}", self.subject_prefix, event_type);

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, &event_type);
        span.record(trogon_semconv::attribute::EVENT_ID, &event_id);
        span.record(trogon_semconv::attribute::SUBJECT, &subject);

        let mut nats_headers = async_nats::HeaderMap::new();
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, event_id.as_str());
        nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
        nats_headers.insert(NATS_HEADER_EVENT_ID, event_id.as_str());
        nats_headers.insert(NATS_HEADER_TEAM_ID, team_id.as_str());
        nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, "event");

        let outcome = self
            .publisher
            .publish_event(subject, nats_headers, body.clone(), self.nats_ack_timeout.into())
            .await;

        (outcome_to_status(outcome), String::new())
    }

    async fn handle_form_payload(&self, body: &Bytes) -> (StatusCode, String) {
        let form_str = match std::str::from_utf8(body) {
            Ok(s) => s,
            Err(_) => {
                warn!("Invalid UTF-8 in form payload");
                self.publish_unroutable("invalid_utf8_form", body.clone()).await;
                return (StatusCode::BAD_REQUEST, String::new());
            }
        };

        let fields: Vec<(String, String)> = form_urlencoded::parse(form_str.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        let find_field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());

        if let Some(payload_json) = find_field("payload") {
            let status = self.handle_interaction(payload_json, body.clone()).await;
            return (status, String::new());
        }

        if let Some(command) = find_field("command") {
            let status = self.handle_slash_command(command, &fields, body.clone()).await;
            return (status, String::new());
        }

        warn!("Unrecognized form payload");
        self.publish_unroutable("unrecognized_form", body.clone()).await;
        (StatusCode::BAD_REQUEST, String::new())
    }

    async fn handle_interaction(&self, payload_json: &str, raw_body: Bytes) -> StatusCode {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_json) else {
            warn!("Invalid JSON in interaction payload field");
            self.publish_unroutable("invalid_interaction_json", raw_body).await;
            return StatusCode::BAD_REQUEST;
        };

        self.publish_interaction_payload(&payload, Bytes::from(payload_json.to_owned()))
            .await
    }

    async fn publish_interaction_payload(&self, payload: &serde_json::Value, body: Bytes) -> StatusCode {
        let Some(interaction_type) = payload.get("type").and_then(|v| v.as_str()) else {
            warn!("Missing type in interaction payload");
            self.publish_unroutable("missing_interaction_type", body).await;
            return StatusCode::BAD_REQUEST;
        };
        let interaction_type = interaction_type.to_owned();

        let Some(trigger_id) = payload.get("trigger_id").and_then(|v| v.as_str()) else {
            warn!("Missing trigger_id in interaction payload");
            self.publish_unroutable("missing_interaction_trigger_id", body).await;
            return StatusCode::BAD_REQUEST;
        };
        let trigger_id = trigger_id.to_owned();

        let team_id = payload
            .get("team")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();

        let subject = format!("{}.interaction.{}", self.subject_prefix, interaction_type);

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, &interaction_type);
        span.record(trogon_semconv::attribute::EVENT_ID, &trigger_id);
        span.record(trogon_semconv::attribute::SUBJECT, &subject);

        info!(interaction_type, "Received Slack interaction");

        let mut nats_headers = async_nats::HeaderMap::new();
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, trigger_id.as_str());
        nats_headers.insert(NATS_HEADER_EVENT_TYPE, interaction_type.as_str());
        nats_headers.insert(NATS_HEADER_TEAM_ID, team_id.as_str());
        nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, "interaction");

        let outcome = self
            .publisher
            .publish_event(subject, nats_headers, body, self.nats_ack_timeout.into())
            .await;

        outcome_to_status(outcome)
    }

    async fn handle_slash_command(&self, command: &str, fields: &[(String, String)], raw_body: Bytes) -> StatusCode {
        let team_id = fields
            .iter()
            .find(|(k, _)| k == "team_id")
            .map(|(_, v)| v.as_str())
            .unwrap_or("unknown");

        let Some(trigger_id) = fields.iter().find(|(k, _)| k == "trigger_id").map(|(_, v)| v.as_str()) else {
            warn!(command, "Missing trigger_id in slash command payload");
            self.publish_unroutable("missing_command_trigger_id", raw_body).await;
            return StatusCode::BAD_REQUEST;
        };

        self.publish_slash_command(command, team_id, trigger_id, raw_body).await
    }

    async fn handle_slash_command_value(&self, payload: &serde_json::Value, raw_body: Bytes) -> StatusCode {
        let command = payload.get("command").and_then(|v| v.as_str()).unwrap_or_default();
        if command.is_empty() {
            warn!("Missing command in slash command payload");
            self.publish_unroutable("missing_command", raw_body).await;
            return StatusCode::BAD_REQUEST;
        }
        let team_id = payload.get("team_id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let Some(trigger_id) = payload.get("trigger_id").and_then(|v| v.as_str()) else {
            warn!(command, "Missing trigger_id in slash command payload");
            self.publish_unroutable("missing_command_trigger_id", raw_body).await;
            return StatusCode::BAD_REQUEST;
        };

        self.publish_slash_command(command, team_id, trigger_id, raw_body).await
    }

    async fn publish_slash_command(
        &self,
        command: &str,
        team_id: &str,
        trigger_id: &str,
        raw_body: Bytes,
    ) -> StatusCode {
        let command_name = command.trim_start_matches('/');
        let subject = format!("{}.command.{}", self.subject_prefix, command_name);

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, command);
        span.record(trogon_semconv::attribute::EVENT_ID, trigger_id);
        span.record(trogon_semconv::attribute::SUBJECT, &subject);

        info!(command, "Received Slack slash command");

        let mut nats_headers = async_nats::HeaderMap::new();
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, trigger_id);
        nats_headers.insert(NATS_HEADER_EVENT_TYPE, command);
        nats_headers.insert(NATS_HEADER_TEAM_ID, team_id);
        nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, "command");

        let outcome = self
            .publisher
            .publish_event(subject, nats_headers, raw_body, self.nats_ack_timeout.into())
            .await;

        outcome_to_status(outcome)
    }
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &SlackConfig) -> Result<(), C::Error> {
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
    config: &SlackConfig,
    webhook: &SlackWebhookConfig,
) -> Router {
    router_with_clock(publisher, config, webhook, SystemClock)
}

fn router_with_clock<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &SlackConfig,
    webhook: &SlackWebhookConfig,
    clock: C,
) -> Router {
    let state = AppState {
        bridge: SlackBridge::new(publisher, config),
        clock,
        signing_secret: webhook.signing_secret.clone(),
        timestamp_max_drift: webhook.timestamp_max_drift,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S, C>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    State(state): State<AppState<P, S, C>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = (StatusCode, String)> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

#[instrument(
    name = SLACK_WEBHOOK,
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        event_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    state: AppState<P, S, C>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    let Some(timestamp) = headers.get(HEADER_TIMESTAMP).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-Slack-Request-Timestamp header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    let Ok(ts) = timestamp.parse::<u64>() else {
        warn!("Invalid X-Slack-Request-Timestamp header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    let now = state
        .clock
        .system_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let drift = now.abs_diff(ts);
    if drift > Duration::from(state.timestamp_max_drift).as_secs() {
        warn!(drift_secs = drift, "Slack request timestamp too old");
        return (StatusCode::UNAUTHORIZED, String::new());
    }

    let Some(sig) = headers.get(HEADER_SIGNATURE).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-Slack-Signature header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    if let Err(e) = signature::verify(state.signing_secret.as_str(), timestamp, &body, sig) {
        warn!(reason = %e, "Slack signature validation failed");
        return (StatusCode::UNAUTHORIZED, String::new());
    }

    let is_form = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with(CONTENT_TYPE_FORM));

    if is_form {
        state.bridge.handle_form_payload(&body).await
    } else {
        state.bridge.handle_json_body(body).await
    }
}

#[cfg(test)]
mod tests;
