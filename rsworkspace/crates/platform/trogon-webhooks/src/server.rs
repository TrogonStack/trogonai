use axum::Router;
use trogon_nats::DottedNatsToken;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};
use trogon_std::NonZeroDuration;

use crate::{EndpointLookup, WebhookVerifier};

pub fn router<R, V, P, S>(
    _registry: R,
    _verifier: V,
    _publisher: ClaimCheckPublisher<P, S>,
    _subject_prefix: DottedNatsToken,
    _ack_timeout: NonZeroDuration,
) -> Router
where
    R: EndpointLookup,
    V: WebhookVerifier,
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    Router::new()
}
