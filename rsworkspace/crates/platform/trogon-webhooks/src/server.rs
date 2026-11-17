use std::error::Error as _;
use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::{Router, routing::post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use buffa::{Message as _, MessageField, MessageName as _};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use sha2::{Digest as _, Sha256};
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut, PublishOutcome};
use trogon_nats::{DottedNatsToken, MAX_SUBJECT_BYTES};
use trogon_std::NonZeroDuration;
use trogonai_proto::gateway::webhooks::v1;

use crate::headers::single_header;
use crate::{AuthenticationError, DeliveryId, EndpointId, EndpointLookup, EndpointStatus, WebhookVerifier};

#[derive(Clone)]
struct Ingress<R, V, P, S> {
    registry: R,
    verifier: V,
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: DottedNatsToken,
    admission_timeout: NonZeroDuration,
    receipt_clock: fn() -> Result<Duration, SystemTimeError>,
}

pub fn router<R, V, P, S>(
    registry: R,
    verifier: V,
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: DottedNatsToken,
    admission_timeout: NonZeroDuration,
) -> Router
where
    R: EndpointLookup,
    V: WebhookVerifier,
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    router_with_receipt_clock(
        registry,
        verifier,
        publisher,
        subject_prefix,
        admission_timeout,
        system_receipt_clock,
    )
}

fn system_receipt_clock() -> Result<Duration, SystemTimeError> {
    SystemTime::now().duration_since(UNIX_EPOCH)
}

fn router_with_receipt_clock<R, V, P, S>(
    registry: R,
    verifier: V,
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: DottedNatsToken,
    admission_timeout: NonZeroDuration,
    receipt_clock: fn() -> Result<Duration, SystemTimeError>,
) -> Router
where
    R: EndpointLookup,
    V: WebhookVerifier,
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    Router::new()
        .route("/webhooks/{endpoint_id}", post(handle::<R, V, P, S>))
        .with_state(Ingress {
            registry,
            verifier,
            publisher,
            subject_prefix,
            admission_timeout,
            receipt_clock,
        })
}

async fn handle<R, V, P, S>(
    State(ingress): State<Ingress<R, V, P, S>>,
    Path(endpoint_id): Path<String>,
    request: Request<Body>,
) -> StatusCode
where
    R: EndpointLookup,
    V: WebhookVerifier,
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    tokio::time::timeout(ingress.admission_timeout.into(), admit(&ingress, &endpoint_id, request))
        .await
        .unwrap_or(StatusCode::SERVICE_UNAVAILABLE)
}

async fn admit<R, V, P, S>(ingress: &Ingress<R, V, P, S>, endpoint_id: &str, request: Request<Body>) -> StatusCode
where
    R: EndpointLookup,
    V: WebhookVerifier,
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    let Ok(endpoint_id) = EndpointId::new(endpoint_id) else {
        return StatusCode::NOT_FOUND;
    };
    let endpoint = match ingress.registry.lookup(&endpoint_id).await {
        Ok(Some(endpoint)) => endpoint,
        Ok(None) => return StatusCode::NOT_FOUND,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE,
    };
    let registration = endpoint.registration();
    if registration.endpoint_id() != &endpoint_id {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if registration.status() == EndpointStatus::Disabled {
        return StatusCode::NOT_FOUND;
    }
    let (parts, body) = request.into_parts();
    let delivery_id =
        match single_header(&parts.headers, "webhook-id", 128).and_then(|value| DeliveryId::new(value).ok()) {
            Some(value) => value,
            None => return StatusCode::BAD_REQUEST,
        };
    let content_type = if parts.headers.contains_key("content-type") {
        match single_header(&parts.headers, "content-type", 256) {
            Some(value) if !value.bytes().any(|byte| byte.is_ascii_control()) => Some(value.to_owned()),
            _ => return StatusCode::BAD_REQUEST,
        }
    } else {
        None
    };
    let body = match to_bytes(body, registration.max_body_size().get()).await {
        Ok(body) => body,
        Err(error)
            if error
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>()) =>
        {
            return StatusCode::PAYLOAD_TOO_LARGE;
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE,
    };
    match ingress.verifier.verify(&endpoint, &parts.headers, &body).await {
        Ok(()) => {}
        Err(AuthenticationError::Unauthorized) => return StatusCode::UNAUTHORIZED,
        Err(AuthenticationError::Unavailable | AuthenticationError::UnavailableWithSource(_)) => {
            return StatusCode::SERVICE_UNAVAILABLE;
        }
    }
    let subject = format!(
        "{}.{}.{}.received",
        ingress.subject_prefix,
        registration.project_id(),
        endpoint_id
    );
    if subject.len() > MAX_SUBJECT_BYTES || subject.bytes().any(|byte| byte.is_ascii_control()) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let now = match (ingress.receipt_clock)() {
        Ok(now) => now,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE,
    };
    let Ok(seconds) = i64::try_from(now.as_secs()) else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let event = v1::WebhookReceived {
        endpoint_id: endpoint_id.as_str().to_owned(),
        project_id: registration.project_id().as_str().to_owned(),
        configuration_revision: endpoint.configuration_revision().get(),
        delivery_id: delivery_id.as_str().to_owned(),
        received_at: MessageField::some(Timestamp::from_unix(seconds, now.subsec_nanos() as i32)),
        body: body.to_vec(),
        content_type,
    };
    let mut digest = Sha256::new();
    for field in [
        registration.project_id().as_str(),
        endpoint_id.as_str(),
        delivery_id.as_str(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    let message_id = URL_SAFE_NO_PAD.encode(digest.finalize());
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(async_nats::header::NATS_MESSAGE_ID, message_id.as_str());
    headers.insert("Content-Type", "application/protobuf");
    headers.insert("Trogon-Event-Type", v1::WebhookReceived::FULL_NAME);
    match ingress
        .publisher
        .publish_event(
            subject,
            headers,
            Bytes::from(event.encode_to_vec()),
            ingress.admission_timeout.into(),
        )
        .await
    {
        PublishOutcome::Published(_) => StatusCode::ACCEPTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests;
