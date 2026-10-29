use std::fmt;
use std::time::Duration;

use super::ack_budget::AckBudget;
use super::config::SlackConfig;
use super::config::SlackSigningSecret;
use super::config::SlackTransport;
use super::config::SlackWebhookConfig;
use super::constants::{
    CONTENT_TYPE_FORM, CONTENT_TYPE_UNKNOWN, HEADER_SIGNATURE, HEADER_TIMESTAMP, NATS_HEADER_CONTENT_TYPE,
    NATS_HEADER_EVENT_ID, NATS_HEADER_EVENT_TYPE, NATS_HEADER_PAYLOAD_KIND, NATS_HEADER_REJECT_REASON,
    NATS_HEADER_TEAM_ID, NATS_HEADER_TRANSPORT, UNKNOWN_TEAM_ID,
};
use super::dedup_window::DedupWindow;
use super::delivery_metadata::{DeliveryContext, DeliveryMetadata};
use super::header_text::{DeliveryHeaders, HeaderText, InvalidHeaderTextError};
use super::interaction_key::InteractionKey;
use super::payload_encoding::PayloadEncoding;
use super::payload_kind::PayloadKind;
use super::payload_metadata::PayloadMetadata;
use super::signature;
use super::ssl_check::SslCheck;
use super::subject::SlackSubject;
use super::system_notice::{SystemMessageId, SystemNoticeType};
use super::unroutable::{UnroutableMessageId, UnroutableReason};
use trogon_std::NonZeroDuration;
use trogon_std::SystemClock;
use trogon_std::time::EpochClock;

use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use form_urlencoded;
use serde_json::value::RawValue;
use std::future::Future;
use std::pin::Pin;
use tracing::{info, instrument, warn};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, ProvisionedStreamField, PublishOutcome,
};
use trogon_nats::{NatsToken, SubjectTokenViolationError};
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

fn delivery_headers(
    transport: SlackTransport,
    kind: PayloadKind,
    message_id: &str,
    event_type: &str,
    event_id: Option<&str>,
    team_id: &str,
    metadata: &DeliveryMetadata,
) -> Result<async_nats::HeaderMap, InvalidHeaderTextError> {
    let headers = DeliveryHeaders::new()
        .with(async_nats::header::NATS_MESSAGE_ID, &HeaderText::new(message_id)?)
        .with(NATS_HEADER_EVENT_TYPE, &HeaderText::new(event_type)?)
        .with(NATS_HEADER_TEAM_ID, &HeaderText::new(team_id)?)
        .with_static(NATS_HEADER_PAYLOAD_KIND, kind.as_str())
        .with_static(NATS_HEADER_CONTENT_TYPE, kind.encoding().content_type())
        .with_static(NATS_HEADER_TRANSPORT, transport.as_str());
    let headers = headers.with_optional(NATS_HEADER_EVENT_ID, event_id)?;
    Ok(metadata.apply(headers)?.into_inner())
}

fn json_team_id(payload: &serde_json::Value) -> &str {
    payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or(UNKNOWN_TEAM_ID)
}

fn interaction_team_id(payload: &serde_json::Value) -> &str {
    payload
        .get("team")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(UNKNOWN_TEAM_ID)
}

fn form_field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[derive(Clone)]
pub struct SlackBridge<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: NatsToken,
    ack_budget: AckBudget,
    transport: SlackTransport,
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
            ack_budget: config.ack_budget,
            transport: config.transport_kind(),
        }
    }

    pub async fn handle_json_body(&self, body: Bytes, context: &DeliveryContext) -> (StatusCode, String) {
        let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
            warn!("Invalid JSON payload");
            let status = self
                .reject(
                    UnroutableReason::InvalidJson,
                    UNKNOWN_TEAM_ID,
                    body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
            return (status, String::new());
        };

        self.handle_json_payload(&body, &payload, context).await
    }

    pub async fn handle_socket_interaction(&self, payload: &RawValue, context: &DeliveryContext) -> StatusCode {
        self.handle_interaction(payload.get(), Bytes::copy_from_slice(payload.get().as_bytes()), context)
            .await
    }

    pub async fn handle_socket_slash_command(&self, payload: &RawValue, context: &DeliveryContext) -> StatusCode {
        let raw_body = Bytes::copy_from_slice(payload.get().as_bytes());
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload.get()) else {
            warn!("Invalid JSON in slash command payload");
            return self
                .reject(
                    UnroutableReason::InvalidJson,
                    UNKNOWN_TEAM_ID,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };
        self.handle_slash_command_value(&value, raw_body, context).await
    }

    pub async fn publish_socket_unroutable(&self, reason: UnroutableReason, body: Bytes) -> StatusCode {
        self.reject(reason, UNKNOWN_TEAM_ID, body, StatusCode::OK).await
    }

    async fn publish_unroutable(
        &self,
        reason: UnroutableReason,
        team_id: &str,
        body: Bytes,
    ) -> PublishOutcome<P::PublishError> {
        let subject = SlackSubject::unroutable(&self.subject_prefix).into_string();
        let message_id = UnroutableMessageId::new(reason, &body);
        let headers = DeliveryHeaders::new()
            .with(async_nats::header::NATS_MESSAGE_ID, message_id.header())
            .with_static(NATS_HEADER_REJECT_REASON, reason.as_str())
            .with_static(NATS_HEADER_PAYLOAD_KIND, "unroutable")
            .with_static(NATS_HEADER_CONTENT_TYPE, CONTENT_TYPE_UNKNOWN)
            .with(NATS_HEADER_TEAM_ID, &HeaderText::or_unknown(team_id))
            .with_static(NATS_HEADER_TRANSPORT, self.transport.as_str())
            .into_inner();

        self.publish(subject, headers, body).await
    }

    async fn publish(
        &self,
        subject: String,
        headers: async_nats::HeaderMap,
        body: Bytes,
    ) -> PublishOutcome<P::PublishError> {
        let limit = self.ack_budget.limit();
        match tokio::time::timeout(limit, self.publisher.publish_event(subject, headers, body, limit)).await {
            Ok(outcome) => outcome,
            Err(_) => PublishOutcome::AckTimedOut(limit),
        }
    }

    async fn reject(&self, reason: UnroutableReason, team_id: &str, body: Bytes, stored: StatusCode) -> StatusCode {
        let outcome = self.publish_unroutable(reason, team_id, body).await;
        if outcome.is_ok() {
            stored
        } else {
            outcome.log_on_error("slack.unroutable");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }

    async fn handle_json_payload(
        &self,
        body: &Bytes,
        payload: &serde_json::Value,
        context: &DeliveryContext,
    ) -> (StatusCode, String) {
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

        let team_id = json_team_id(payload).to_owned();

        if let Some(notice) = SystemNoticeType::from_payload_type(payload_type) {
            let status = self
                .publish_system_notice(notice, &team_id, payload, body.clone(), context)
                .await;
            return (status, String::new());
        }

        if payload_type != "event_callback" {
            warn!(payload_type, "Unhandled payload type");
            let status = self
                .reject(
                    UnroutableReason::UnhandledPayloadType,
                    &team_id,
                    body.clone(),
                    StatusCode::OK,
                )
                .await;
            return (status, String::new());
        }

        let Some(event_type) = payload
            .get("event")
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str())
        else {
            warn!("Missing event.type in event_callback payload");
            let status = self
                .reject(
                    UnroutableReason::MissingEventType,
                    &team_id,
                    body.clone(),
                    StatusCode::BAD_REQUEST,
                )
                .await;
            return (status, String::new());
        };
        let event_type = event_type.to_owned();

        let Some(event_id) = payload.get("event_id").and_then(|v| v.as_str()) else {
            warn!("Missing event_id in event_callback payload");
            let status = self
                .reject(
                    UnroutableReason::MissingEventId,
                    &team_id,
                    body.clone(),
                    StatusCode::BAD_REQUEST,
                )
                .await;
            return (status, String::new());
        };
        let event_id = event_id.to_owned();

        let subject = match SlackSubject::event(&self.subject_prefix, &event_type) {
            Ok(subject) => subject,
            Err(error) => {
                warn!(%error, "Event type is not a valid subject token");
                let status = self
                    .reject(
                        UnroutableReason::InvalidSubjectToken,
                        &team_id,
                        body.clone(),
                        StatusCode::BAD_REQUEST,
                    )
                    .await;
                return (status, String::new());
            }
        };

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, &event_type);
        span.record(trogon_semconv::attribute::EVENT_ID, &event_id);
        span.record(trogon_semconv::attribute::SUBJECT, subject.as_str());

        let Ok(nats_headers) = delivery_headers(
            self.transport,
            PayloadKind::Event,
            &event_id,
            &event_type,
            Some(&event_id),
            &team_id,
            &DeliveryMetadata::new(context, PayloadMetadata::event(payload)),
        ) else {
            warn!("Event metadata is not valid header text");
            let status = self
                .reject(
                    UnroutableReason::InvalidHeaderValue,
                    &team_id,
                    body.clone(),
                    StatusCode::BAD_REQUEST,
                )
                .await;
            return (status, String::new());
        };

        let outcome = self.publish(subject.into_string(), nats_headers, body.clone()).await;

        (outcome_to_status(outcome), String::new())
    }

    async fn publish_system_notice(
        &self,
        notice: SystemNoticeType,
        team_id: &str,
        payload: &serde_json::Value,
        body: Bytes,
        context: &DeliveryContext,
    ) -> StatusCode {
        let subject = SlackSubject::system(&self.subject_prefix, notice);
        let message_id = SystemMessageId::new(&body);
        let notice_type = notice.as_str();

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, notice_type);
        span.record(trogon_semconv::attribute::SUBJECT, subject.as_str());

        info!(notice_type, "Received Slack system notice");

        let Ok(nats_headers) = delivery_headers(
            self.transport,
            PayloadKind::System,
            message_id.as_str(),
            notice_type,
            None,
            team_id,
            &DeliveryMetadata::new(context, PayloadMetadata::system(payload)),
        ) else {
            warn!("System notice metadata is not valid header text");
            return self
                .reject(
                    UnroutableReason::InvalidHeaderValue,
                    team_id,
                    body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };

        let outcome = self.publish(subject.into_string(), nats_headers, body).await;

        outcome_to_status(outcome)
    }

    async fn handle_form_payload(&self, body: &Bytes, context: &DeliveryContext) -> (StatusCode, String) {
        let form_str = match std::str::from_utf8(body) {
            Ok(s) => s,
            Err(_) => {
                warn!("Invalid UTF-8 in form payload");
                let status = self
                    .reject(
                        UnroutableReason::InvalidUtf8Form,
                        UNKNOWN_TEAM_ID,
                        body.clone(),
                        StatusCode::BAD_REQUEST,
                    )
                    .await;
                return (status, String::new());
            }
        };

        let fields: Vec<(String, String)> = form_urlencoded::parse(form_str.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        if let Some(payload_json) = form_field(&fields, "payload") {
            let status = self.handle_interaction(payload_json, body.clone(), context).await;
            return (status, String::new());
        }

        if let Some(command) = form_field(&fields, "command") {
            let status = self.handle_slash_command(command, &fields, body.clone(), context).await;
            return (status, String::new());
        }

        warn!("Unrecognized form payload");
        let team_id = form_field(&fields, "team_id").unwrap_or(UNKNOWN_TEAM_ID);
        let status = self
            .reject(
                UnroutableReason::UnrecognizedForm,
                team_id,
                body.clone(),
                StatusCode::BAD_REQUEST,
            )
            .await;
        (status, String::new())
    }

    async fn handle_interaction(&self, payload_json: &str, raw_body: Bytes, context: &DeliveryContext) -> StatusCode {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_json) else {
            warn!("Invalid JSON in interaction payload field");
            return self
                .reject(
                    UnroutableReason::InvalidInteractionJson,
                    UNKNOWN_TEAM_ID,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };

        self.publish_interaction_payload(&payload, Bytes::from(payload_json.to_owned()), context)
            .await
    }

    async fn publish_interaction_payload(
        &self,
        payload: &serde_json::Value,
        body: Bytes,
        context: &DeliveryContext,
    ) -> StatusCode {
        let team_id = interaction_team_id(payload).to_owned();

        let Some(interaction_type) = payload.get("type").and_then(|v| v.as_str()) else {
            warn!("Missing type in interaction payload");
            return self
                .reject(
                    UnroutableReason::MissingInteractionType,
                    &team_id,
                    body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };
        let interaction_type = interaction_type.to_owned();

        let key = InteractionKey::derive(payload, &interaction_type, &body);

        let subject = match SlackSubject::interaction(&self.subject_prefix, &interaction_type) {
            Ok(subject) => subject,
            Err(error) => {
                warn!(%error, "Interaction type is not a valid subject token");
                return self
                    .reject(
                        UnroutableReason::InvalidSubjectToken,
                        &team_id,
                        body,
                        StatusCode::BAD_REQUEST,
                    )
                    .await;
            }
        };

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, &interaction_type);
        span.record(trogon_semconv::attribute::EVENT_ID, key.as_str());
        span.record(trogon_semconv::attribute::SUBJECT, subject.as_str());

        info!(interaction_type, "Received Slack interaction");

        let Ok(nats_headers) = delivery_headers(
            self.transport,
            PayloadKind::Interaction,
            key.as_str(),
            &interaction_type,
            None,
            &team_id,
            &DeliveryMetadata::new(context, PayloadMetadata::interaction(payload)),
        ) else {
            warn!("Interaction metadata is not valid header text");
            return self
                .reject(
                    UnroutableReason::InvalidHeaderValue,
                    &team_id,
                    body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };

        let outcome = self.publish(subject.into_string(), nats_headers, body).await;

        outcome_to_status(outcome)
    }

    async fn handle_slash_command(
        &self,
        command: &str,
        fields: &[(String, String)],
        raw_body: Bytes,
        context: &DeliveryContext,
    ) -> StatusCode {
        let team_id = form_field(fields, "team_id").unwrap_or(UNKNOWN_TEAM_ID);

        if command.is_empty() {
            warn!("Missing command in slash command payload");
            return self
                .reject(
                    UnroutableReason::MissingCommand,
                    team_id,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        }

        let Some(trigger_id) = form_field(fields, "trigger_id") else {
            warn!(command, "Missing trigger_id in slash command payload");
            return self
                .reject(
                    UnroutableReason::MissingCommandTriggerId,
                    team_id,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };

        let metadata = DeliveryMetadata::new(context, PayloadMetadata::command_fields(fields));
        self.publish_slash_command(command, team_id, trigger_id, raw_body, PayloadEncoding::Form, metadata)
            .await
    }

    async fn handle_slash_command_value(
        &self,
        payload: &serde_json::Value,
        raw_body: Bytes,
        context: &DeliveryContext,
    ) -> StatusCode {
        let team_id = json_team_id(payload);
        let command = payload.get("command").and_then(|v| v.as_str()).unwrap_or_default();
        if command.is_empty() {
            warn!("Missing command in slash command payload");
            return self
                .reject(
                    UnroutableReason::MissingCommand,
                    team_id,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        }
        let Some(trigger_id) = payload.get("trigger_id").and_then(|v| v.as_str()) else {
            warn!(command, "Missing trigger_id in slash command payload");
            return self
                .reject(
                    UnroutableReason::MissingCommandTriggerId,
                    team_id,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };

        let metadata = DeliveryMetadata::new(context, PayloadMetadata::command_value(payload));
        self.publish_slash_command(command, team_id, trigger_id, raw_body, PayloadEncoding::Json, metadata)
            .await
    }

    async fn publish_slash_command(
        &self,
        command: &str,
        team_id: &str,
        trigger_id: &str,
        raw_body: Bytes,
        encoding: PayloadEncoding,
        metadata: DeliveryMetadata<'_>,
    ) -> StatusCode {
        let subject = match SlackSubject::command(&self.subject_prefix, command) {
            Ok(subject) => subject,
            Err(error) => {
                warn!(%error, "Command name is not a valid subject token");
                let reason = match error {
                    SubjectTokenViolationError::Empty => UnroutableReason::MissingCommand,
                    _ => UnroutableReason::InvalidSubjectToken,
                };
                return self.reject(reason, team_id, raw_body, StatusCode::BAD_REQUEST).await;
            }
        };

        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, command);
        span.record(trogon_semconv::attribute::EVENT_ID, trigger_id);
        span.record(trogon_semconv::attribute::SUBJECT, subject.as_str());

        info!(command, "Received Slack slash command");

        let Ok(nats_headers) = delivery_headers(
            self.transport,
            PayloadKind::Command(encoding),
            trigger_id,
            command,
            None,
            team_id,
            &metadata,
        ) else {
            warn!("Slash command metadata is not valid header text");
            return self
                .reject(
                    UnroutableReason::InvalidHeaderValue,
                    team_id,
                    raw_body,
                    StatusCode::BAD_REQUEST,
                )
                .await;
        };

        let outcome = self.publish(subject.into_string(), nats_headers, raw_body).await;

        outcome_to_status(outcome)
    }
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &SlackConfig) -> Result<(), C::Error> {
    let dedup_window = DedupWindow::within(config.stream_max_age);
    js.create_or_reconcile_stream(
        async_nats::jetstream::stream::Config {
            name: config.stream_name.to_string(),
            subjects: vec![format!("{}.>", config.subject_prefix)],
            max_age: config.stream_max_age.into(),
            duplicate_window: dedup_window.into(),
            ..Default::default()
        },
        &[
            ProvisionedStreamField::Subjects,
            ProvisionedStreamField::DuplicateWindow,
            ProvisionedStreamField::MaxAge,
        ],
    )
    .await?;

    if let Some(requested) = config.ack_budget.capped_from() {
        let requested_secs = Duration::from(requested).as_secs();
        let effective_millis = config.ack_budget.limit().as_millis() as u64;
        warn!(
            requested_secs,
            effective_millis,
            "nats_ack_timeout_secs exceeds the time Slack allows for an acknowledgement and was lowered"
        );
    }

    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    let stream_name = config.stream_name.as_str();
    let dedup_window_secs = Duration::from(dedup_window).as_secs();
    info!(
        stream = stream_name,
        max_age_secs, dedup_window_secs, "JetStream stream ready"
    );
    Ok(())
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &SlackConfig,
    webhook: &SlackWebhookConfig,
) -> Router {
    router_with_clock(publisher, config, webhook, SystemClock)
}

pub(super) fn router_with_clock<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
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
        .layer(DefaultBodyLimit::max(webhook.max_body_size.as_usize()))
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
    let content_type = headers.get("content-type").and_then(|v| v.to_str().ok());
    if SslCheck::detect(content_type, &body) == SslCheck::Requested {
        info!("Responding to Slack ssl_check");
        return (StatusCode::OK, String::new());
    }

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

    let context = DeliveryContext::http(&headers);

    if is_form {
        state.bridge.handle_form_payload(&body, &context).await
    } else {
        state.bridge.handle_json_body(body, &context).await
    }
}

#[cfg(test)]
mod tests;
