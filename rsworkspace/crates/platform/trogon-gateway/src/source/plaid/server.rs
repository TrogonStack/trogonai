use std::fmt;
use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::DefaultBodyLimit,
    extract::State,
    http::HeaderMap,
    http::StatusCode,
    http::header,
    response::{IntoResponse, Response},
    routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, ProvisionedStreamField, PublishOutcome,
};
use trogon_semconv::span::PLAID_WEBHOOK;
use trogon_std::time::EpochClock;
use trogon_std::{NonZeroDuration, SystemClock};

use super::PlaidConfig;
use super::PlaidDeliveryId;
use super::PlaidEventType;
use super::PlaidItemId;
use super::PlaidKeyId;
use super::PlaidSecret;
use super::constants::{
    HEADER_VERIFICATION, HTTP_BODY_SIZE_MAX, KEY_FETCH_THROTTLE_INTERVAL, NATS_HEADER_EVENT_TYPE, NATS_HEADER_ITEM_ID,
    NATS_HEADER_REJECT_REASON,
};
use super::dedup_window::DedupWindow;
use super::key_cache::{PlaidKeyCache, PlaidKeyResolveError};
use super::key_fetcher::{PlaidKeyFetchError, PlaidKeyFetcher, ReqwestPlaidKeyFetcher};
use super::signature;
use crate::credential::commands::domain::{CredentialKind, SourceKind};
use crate::credential_runtime_projection::{RuntimeCredentialResolver, RuntimeIntegrationKey};
use crate::secret_store::{SecretStoreError, SecretStoreGet};
use crate::source_integration_id::SourceIntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingWebhookType,
    MissingWebhookCode,
    InvalidEventType,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingWebhookType => "missing_webhook_type",
            Self::MissingWebhookCode => "missing_webhook_code",
            Self::InvalidEventType => "invalid_event_type",
        }
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Plaid event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("plaid");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

// Plaid waits out `Retry-After` on a 429 instead of backing off exponentially,
// so a webhook whose key fetch was throttled is retried once a fetch is allowed again.
fn key_fetch_throttled() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, KEY_FETCH_THROTTLE_INTERVAL.as_secs())],
    )
        .into_response()
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    delivery_id: &PlaidDeliveryId,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: NonZeroDuration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(async_nats::header::NATS_MESSAGE_ID, delivery_id.as_str());
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout.into())
        .await;
    if outcome.is_ok() {
        StatusCode::OK
    } else {
        outcome.log_on_error("plaid.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct Ingest<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock, F: PlaidKeyFetcher> {
    publisher: ClaimCheckPublisher<P, S>,
    clock: C,
    keys: PlaidKeyCache<F>,
    subject_prefix: NatsToken,
    timestamp_tolerance: NonZeroDuration,
    nats_ack_timeout: NonZeroDuration,
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock, F: PlaidKeyFetcher> {
    ingest: Ingest<P, S, C, F>,
    secret: PlaidSecret,
}

#[derive(Clone)]
struct RuntimeAppState<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock, F: PlaidKeyFetcher, G> {
    ingest: Ingest<P, S, C, F>,
    credential_resolver: RuntimeCredentialResolver<G>,
    runtime_key: RuntimeIntegrationKey,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &PlaidConfig) -> Result<(), C::Error> {
    // A token verifies for the tolerance on either side of its `iat`, so replays
    // of one delivery can arrive up to twice the tolerance apart.
    let dedup_window = DedupWindow::spanning(config.timestamp_tolerance, config.stream_max_age);
    js.create_or_reconcile_stream(
        async_nats::jetstream::stream::Config {
            name: config.stream_name.as_str().to_owned(),
            subjects: vec![format!("{}.>", config.subject_prefix)],
            duplicate_window: dedup_window.into(),
            max_age: config.stream_max_age.into(),
            ..Default::default()
        },
        &[
            ProvisionedStreamField::Subjects,
            ProvisionedStreamField::DuplicateWindow,
            ProvisionedStreamField::MaxAge,
        ],
    )
    .await?;

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
    config: &PlaidConfig,
) -> Router {
    let fetcher = ReqwestPlaidKeyFetcher::new(&config.api_base_url, config.client_id.clone());
    router_with_clock(publisher, config, SystemClock, fetcher)
}

pub fn runtime_router<P, S, G>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &PlaidConfig,
    integration_id: SourceIntegrationId,
    credential_resolver: RuntimeCredentialResolver<G>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let fetcher = ReqwestPlaidKeyFetcher::new(&config.api_base_url, config.client_id.clone());
    runtime_router_with_clock(
        publisher,
        config,
        integration_id,
        credential_resolver,
        SystemClock,
        fetcher,
    )
}

fn router_with_clock<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock, F: PlaidKeyFetcher>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &PlaidConfig,
    clock: C,
    fetcher: F,
) -> Router {
    let state = AppState {
        ingest: Ingest::new(publisher, config, clock, fetcher),
        secret: config.secret.clone(),
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S, C, F>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn runtime_router_with_clock<P, S, C, F, G>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &PlaidConfig,
    integration_id: SourceIntegrationId,
    credential_resolver: RuntimeCredentialResolver<G>,
    clock: C,
    fetcher: F,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    C: EpochClock,
    F: PlaidKeyFetcher,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let state = RuntimeAppState {
        ingest: Ingest::new(publisher, config, clock, fetcher),
        credential_resolver,
        runtime_key: RuntimeIntegrationKey::new(SourceKind::Plaid, &integration_id),
    };

    Router::new()
        .route("/webhook", post(handle_runtime_webhook::<P, S, C, F, G>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn verification_token(headers: &HeaderMap) -> Result<(&str, PlaidKeyId), StatusCode> {
    let Some(token) = header_str(headers, HEADER_VERIFICATION) else {
        warn!("Missing Plaid-Verification header");
        return Err(StatusCode::UNAUTHORIZED);
    };

    match signature::key_id(token) {
        Ok(key_id) => Ok((token, key_id)),
        Err(err) => {
            warn!(error = %err, "Invalid Plaid-Verification header");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

#[instrument(
    name = PLAID_WEBHOOK,
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock, F: PlaidKeyFetcher>(
    State(state): State<AppState<P, S, C, F>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (token, key_id) = match verification_token(&headers) {
        Ok(verification) => verification,
        Err(status) => return status.into_response(),
    };

    state
        .ingest
        .verify_and_publish(token, &key_id, &state.secret, body)
        .await
}

#[instrument(
    name = PLAID_WEBHOOK,
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_runtime_webhook<P, S, C, F, G>(
    State(state): State<RuntimeAppState<P, S, C, F, G>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    C: EpochClock,
    F: PlaidKeyFetcher,
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let (token, key_id) = match verification_token(&headers) {
        Ok(verification) => verification,
        Err(status) => return status.into_response(),
    };

    let secret = match state
        .credential_resolver
        .resolve_plaintext(&state.runtime_key, CredentialKind::ClientSecret)
        .await
    {
        Ok(secret) => PlaidSecret::from(secret),
        Err(error) => {
            warn!(reason = %error, "Plaid runtime credential resolution failed");
            return error.http_status().into_response();
        }
    };

    state.ingest.verify_and_publish(token, &key_id, &secret, body).await
}

impl<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock, F: PlaidKeyFetcher> Ingest<P, S, C, F> {
    fn new(publisher: ClaimCheckPublisher<P, S>, config: &PlaidConfig, clock: C, fetcher: F) -> Self {
        Self {
            publisher,
            clock,
            keys: PlaidKeyCache::new(fetcher),
            subject_prefix: config.subject_prefix.clone(),
            timestamp_tolerance: config.timestamp_tolerance,
            nats_ack_timeout: config.nats_ack_timeout,
        }
    }

    async fn verify_and_publish(
        &self,
        token: &str,
        key_id: &PlaidKeyId,
        secret: &PlaidSecret,
        body: Bytes,
    ) -> Response {
        let now = self.clock.system_time();
        let key = match self.keys.resolve(key_id, secret, now).await {
            Ok(key) => key,
            Err(PlaidKeyResolveError::Fetch(PlaidKeyFetchError::UnknownKey)) => {
                warn!(%key_id, "Plaid does not know the verification key id");
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Err(PlaidKeyResolveError::Fetch(err)) => {
                warn!(error = %err, %key_id, "Plaid verification key fetch failed");
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            Err(err @ PlaidKeyResolveError::Throttled(_)) => {
                warn!(error = %err, %key_id, "Plaid verification key fetch throttled");
                return key_fetch_throttled();
            }
            Err(err @ PlaidKeyResolveError::Expired(_)) => {
                warn!(error = %err, %key_id, "Plaid verification key unusable");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        };

        let delivery_id = match signature::verify(token, &key, &body, now, self.timestamp_tolerance.into()) {
            Ok(delivery_id) => delivery_id,
            Err(err) => {
                warn!(error = %err, "Plaid webhook verification failed");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        };

        self.publish_verified(delivery_id, body).await.into_response()
    }

    async fn publish_verified(&self, delivery_id: PlaidDeliveryId, body: Bytes) -> StatusCode {
        let parsed: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(err) => {
                warn!(error = %err, "Failed to parse Plaid webhook body as JSON");
                return publish_unroutable(
                    &self.publisher,
                    &self.subject_prefix,
                    &delivery_id,
                    RejectReason::InvalidJson,
                    body,
                    self.nats_ack_timeout,
                )
                .await;
            }
        };

        let Some(webhook_type) = parsed.get("webhook_type").and_then(|value| value.as_str()) else {
            warn!("Missing webhook_type in Plaid webhook payload");
            return publish_unroutable(
                &self.publisher,
                &self.subject_prefix,
                &delivery_id,
                RejectReason::MissingWebhookType,
                body,
                self.nats_ack_timeout,
            )
            .await;
        };

        let Some(webhook_code) = parsed.get("webhook_code").and_then(|value| value.as_str()) else {
            warn!("Missing webhook_code in Plaid webhook payload");
            return publish_unroutable(
                &self.publisher,
                &self.subject_prefix,
                &delivery_id,
                RejectReason::MissingWebhookCode,
                body,
                self.nats_ack_timeout,
            )
            .await;
        };

        let event_type = match PlaidEventType::from_webhook(webhook_type, webhook_code) {
            Ok(event_type) => event_type,
            Err(err) => {
                warn!(error = %err, "Invalid event type in Plaid webhook payload");
                return publish_unroutable(
                    &self.publisher,
                    &self.subject_prefix,
                    &delivery_id,
                    RejectReason::InvalidEventType,
                    body,
                    self.nats_ack_timeout,
                )
                .await;
            }
        };

        let subject = format!("{}.{}", self.subject_prefix, event_type);
        let span = tracing::Span::current();
        span.record(trogon_semconv::attribute::EVENT_TYPE, event_type.as_str());
        span.record(trogon_semconv::attribute::SUBJECT, &subject);

        let mut nats_headers = async_nats::HeaderMap::new();
        nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, delivery_id.as_str());
        nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
        if let Some(raw_item_id) = parsed.get("item_id").and_then(|value| value.as_str()) {
            match PlaidItemId::new(raw_item_id) {
                Ok(item_id) => nats_headers.insert(NATS_HEADER_ITEM_ID, item_id.as_str()),
                Err(err) => warn!(error = %err, "Ignoring invalid item_id in Plaid webhook payload"),
            }
        }

        let outcome = self
            .publisher
            .publish_event(subject, nats_headers, body, self.nats_ack_timeout.into())
            .await;

        outcome_to_status(outcome)
    }
}

#[cfg(test)]
mod tests;
