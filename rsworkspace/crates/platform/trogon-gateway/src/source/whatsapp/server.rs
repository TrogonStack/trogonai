use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{TryStreamExt, stream};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::warn;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, ProvisionedStreamField, StreamMaxAge,
};

use super::admission::{AdmissionError, admit};
use super::config::WhatsAppConfig;
use super::constants::{DEDUP_WINDOW, PUBLISH_CONCURRENCY};
use crate::source::hub_signature;

#[derive(Clone)]
struct AppState<P, S> {
    config: WhatsAppConfig,
    publisher: ClaimCheckPublisher<P, S>,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &WhatsAppConfig) -> Result<(), C::Error> {
    let duplicate_window = match config.stream_max_age {
        StreamMaxAge::NoExpiry => DEDUP_WINDOW,
        StreamMaxAge::ExpireAfter(age) => DEDUP_WINDOW.min(age.into()),
    };
    js.create_or_reconcile_stream(
        async_nats::jetstream::stream::Config {
            name: config.stream_name.to_string(),
            subjects: vec![format!("{}.>", config.subject_prefix)],
            max_age: config.stream_max_age.into(),
            duplicate_window,
            ..Default::default()
        },
        &[
            ProvisionedStreamField::Subjects,
            ProvisionedStreamField::DuplicateWindow,
            ProvisionedStreamField::MaxAge,
        ],
    )
    .await
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &WhatsAppConfig,
) -> Router {
    let state = AppState {
        config: config.clone(),
        publisher,
    };
    Router::new()
        .route("/webhook", get(verify_callback::<P, S>).post(receive::<P, S>))
        .layer(DefaultBodyLimit::max(config.webhook.max_body_bytes.as_usize()))
        .with_state(state)
}

#[derive(Deserialize)]
struct VerificationQuery {
    #[serde(rename = "hub.mode")]
    _mode: VerificationMode,
    #[serde(rename = "hub.verify_token")]
    verify_token: String,
    #[serde(rename = "hub.challenge")]
    challenge: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum VerificationMode {
    Subscribe,
}

async fn verify_callback<P, S>(
    State(state): State<AppState<P, S>>,
    query: Result<Query<VerificationQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if query.challenge.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !bool::from(
        Sha256::digest(query.verify_token.as_bytes())
            .ct_eq(&Sha256::digest(state.config.webhook.verify_token.as_str().as_bytes())),
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    query.challenge.into_response()
}

async fn receive<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let mut signatures = headers.get_all("x-hub-signature-256").iter();
    let signature = signatures.next().and_then(|signature| signature.to_str().ok());
    match signature {
        Some(signature)
            if signatures.next().is_none()
                && hub_signature::verify(state.config.webhook.app_secret.as_str(), &body, signature).is_ok() => {}
        _ => return StatusCode::UNAUTHORIZED,
    }

    let events = match admit(
        &body,
        &state.config.webhook.waba_id,
        &state.config.webhook.phone_number_id,
    ) {
        Ok(events) => events,
        Err(AdmissionError::TooManyUpdates) => return StatusCode::PAYLOAD_TOO_LARGE,
        Err(AdmissionError::ForeignBinding) => return StatusCode::FORBIDDEN,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    let subject = format!("{}.message.received", state.config.subject_prefix);
    let timeout = Duration::from(state.config.nats_ack_timeout);
    let publishing = stream::iter(events.into_iter().map(Ok)).try_for_each_concurrent(PUBLISH_CONCURRENCY, |event| {
        let subject = subject.clone();
        let publisher = &state.publisher;
        async move {
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("Nats-Msg-Id", event.dedup_id());
            let outcome = publisher
                .publish_event(subject, headers, event.payload().into(), timeout)
                .await;
            if outcome.is_ok() { Ok(()) } else { Err(outcome) }
        }
    });
    match tokio::time::timeout(timeout, publishing).await {
        Ok(Ok(())) => StatusCode::OK,
        Ok(Err(outcome)) => {
            outcome.log_on_error("whatsapp");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(_) => {
            warn!("WhatsApp batch publication timed out");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
