use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use tracing::{debug, error};

use super::object_store::{ObjectStoreGet, ObjectStorePut};
use super::publish::PublishOutcome;
use super::traits::JetStreamPublisher;

pub const HEADER_CLAIM_CHECK: &str = "Trogon-Claim-Check";
pub const HEADER_CLAIM_BUCKET: &str = "Trogon-Claim-Bucket";
pub const HEADER_CLAIM_KEY: &str = "Trogon-Claim-Key";

const CLAIM_CHECK_VERSION: &str = "v1";
const PROTOCOL_OVERHEAD: usize = 8 * 1024;
const CLAIM_HEADER_PREFIX: &str = "Trogon-Claim-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxPayload(usize);

impl MaxPayload {
    pub fn from_server_limit(server_max: usize) -> Self {
        Self(server_max.saturating_sub(PROTOCOL_OVERHEAD))
    }

    pub fn threshold(&self) -> usize {
        self.0
    }
}

pub trait MaxPayloadLimit: Send + Sync + 'static {
    fn max_payload(&self) -> MaxPayload;
}

impl MaxPayloadLimit for MaxPayload {
    fn max_payload(&self) -> MaxPayload {
        *self
    }
}

impl MaxPayloadLimit for async_nats::Client {
    fn max_payload(&self) -> MaxPayload {
        MaxPayload::from_server_limit(async_nats::Client::max_payload(self))
    }
}

pub fn is_claim(headers: &HeaderMap) -> bool {
    headers
        .get(HEADER_CLAIM_CHECK)
        .is_some_and(|v| v.as_str() == CLAIM_CHECK_VERSION)
}

pub async fn resolve_claim<S: ObjectStoreGet>(
    headers: &HeaderMap,
    payload: Bytes,
    store: &S,
) -> Result<Bytes, ClaimResolveError<S::Error>> {
    use tokio::io::AsyncReadExt;

    if !is_claim(headers) {
        return Ok(payload);
    }

    let key = headers.get(HEADER_CLAIM_KEY).ok_or(ClaimResolveError::MissingKey)?;

    let mut reader = store.get(key.as_str()).await.map_err(ClaimResolveError::StoreFailed)?;

    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .await
        .map_err(ClaimResolveError::ReadFailed)?;

    Ok(Bytes::from(buf))
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimResolveError<E> {
    #[error("claim message missing {} header", HEADER_CLAIM_KEY)]
    MissingKey,
    #[error("failed to resolve claim from object store: {0}")]
    StoreFailed(#[source] E),
    #[error("failed to read claim payload: {0}")]
    ReadFailed(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct ClaimCheckPublisher<P, S> {
    publisher: P,
    store: S,
    bucket_name: String,
    max_payload: Arc<dyn MaxPayloadLimit>,
}

impl<P: fmt::Debug, S: fmt::Debug> fmt::Debug for ClaimCheckPublisher<P, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimCheckPublisher")
            .field("publisher", &self.publisher)
            .field("store", &self.store)
            .field("bucket_name", &self.bucket_name)
            .finish_non_exhaustive()
    }
}

impl<P, S> ClaimCheckPublisher<P, S> {
    pub fn new<M: MaxPayloadLimit>(publisher: P, store: S, bucket_name: String, max_payload: M) -> Self {
        Self {
            publisher,
            store,
            bucket_name,
            max_payload: Arc::new(max_payload),
        }
    }
}

fn strip_claim_headers(headers: HeaderMap) -> HeaderMap {
    let dominated = headers.iter().any(|(name, _)| {
        let s: &str = name.as_ref();
        s.starts_with(CLAIM_HEADER_PREFIX)
    });

    if !dominated {
        return headers;
    }

    headers
        .iter()
        .filter(|(name, _)| {
            let s: &str = name.as_ref();
            !s.starts_with(CLAIM_HEADER_PREFIX)
        })
        .flat_map(|(name, values)| values.iter().map(move |v| (name.clone(), v.clone())))
        .collect()
}

fn claim_object_key(subject: &str) -> String {
    let id = uuid::Uuid::now_v7();
    format!("{subject}/{id}")
}

impl<P: JetStreamPublisher, S: ObjectStorePut> ClaimCheckPublisher<P, S> {
    pub async fn publish_event(
        &self,
        subject: String,
        headers: HeaderMap,
        payload: Bytes,
        ack_timeout: Duration,
    ) -> PublishOutcome<P::PublishError> {
        let payload_bytes = payload.len();
        let max_payload = self.max_payload.max_payload();
        let threshold = max_payload.threshold();

        if payload_bytes <= threshold {
            debug!(
                nats.subject = %subject,
                messaging.message.body.size = payload_bytes,
                trogon.claim_check.threshold_bytes = threshold,
                trogon.claim_check.used = false,
                "publishing directly"
            );
            return super::publish::publish_event(
                &self.publisher,
                subject,
                strip_claim_headers(headers),
                payload,
                ack_timeout,
            )
            .await;
        }

        let key = claim_object_key(&subject);

        debug!(
            nats.subject = %subject,
            messaging.message.body.size = payload_bytes,
            trogon.claim_check.threshold_bytes = threshold,
            trogon.claim_check.used = true,
            trogon.claim_check.key = %key,
            "payload exceeds threshold, storing in object store"
        );

        // Store-then-publish: if publish fails, the object becomes orphaned.
        // Cleanup relies on the object store bucket's TTL — see #101.
        let mut cursor = std::io::Cursor::new(payload);
        if let Err(e) = self.store.put(&key, &mut cursor).await {
            error!(error = %e, "claim check: failed to store payload in object store");
            return PublishOutcome::StoreFailed(Box::new(e));
        }

        let mut claim_headers = headers;
        claim_headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        claim_headers.insert(HEADER_CLAIM_BUCKET, self.bucket_name.as_str());
        claim_headers.insert(HEADER_CLAIM_KEY, key.as_str());

        super::publish::publish_event(&self.publisher, subject, claim_headers, Bytes::new(), ack_timeout).await
    }
}

#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "test-support"))]
mod integration_tests;
