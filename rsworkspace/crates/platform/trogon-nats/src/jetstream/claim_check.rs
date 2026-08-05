use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tracing::{debug, error};

use crate::constants::{
    CLAIM_CHECK_VERSION, CLAIM_HEADER_PREFIX, HEADER_CLAIM_BUCKET, HEADER_CLAIM_CHECK, HEADER_CLAIM_KEY,
    PROTOCOL_OVERHEAD,
};

use super::claim_bucket::{ClaimBucket, ClaimBucketError, ClaimBucketHeader};
use super::object_store::{ClaimBucketBinding, ObjectStoreGet, ObjectStorePut};
use super::publish::PublishOutcome;
use super::traits::JetStreamPublisher;

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

/// The consumer half of a claim check: an object store bound to the bucket the
/// publisher was configured to write.
///
/// [`resolve_claim`] takes an already-bound store and cannot tell whether it is
/// the right one, so a consumer pointed at the wrong bucket reports every claim
/// as a missing object. Checking the [`HEADER_CLAIM_BUCKET`] the publisher
/// already sends turns that misconfiguration into its own error, which is only
/// worth anything if the name checked against is the name actually opened:
/// hence a [`ClaimBucketBinding`] rather than a store and a name.
#[derive(Debug, Clone)]
pub struct ClaimResolver<S> {
    store: S,
    bucket: ClaimBucket,
}

impl<S: ObjectStoreGet> ClaimResolver<S> {
    pub fn new(binding: ClaimBucketBinding<S>) -> Self {
        let (store, bucket) = binding.into_parts();
        Self { store, bucket }
    }

    pub fn bucket(&self) -> &ClaimBucket {
        &self.bucket
    }

    /// The body a consumer should act on: `payload` itself when the message
    /// carries one, or the stored object when the payload was offloaded.
    /// Headers are optional because that is how a subscription hands them over,
    /// and a message without headers is never a claim.
    pub async fn resolve(
        &self,
        headers: Option<&HeaderMap>,
        payload: Bytes,
    ) -> Result<Bytes, ClaimResolveError<S::Error>> {
        let Some(headers) = headers else {
            return Ok(payload);
        };
        if !is_claim(headers) {
            return Ok(payload);
        }
        if let Some(header) = headers.get(HEADER_CLAIM_BUCKET) {
            let header = ClaimBucketHeader::new(header.as_str());
            let named = match header.parse() {
                Ok(named) => named,
                Err(source) => return Err(ClaimResolveError::UnnamableBucket { named: header, source }),
            };
            if named != self.bucket {
                return Err(ClaimResolveError::BucketMismatch {
                    expected: self.bucket.clone(),
                    named,
                });
            }
        }
        resolve_claim(headers, payload, &self.store).await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimResolveError<E> {
    #[error("claim message missing {} header", HEADER_CLAIM_KEY)]
    MissingKey,
    /// A claim written to a bucket this consumer does not read: both names are
    /// bucket names, they are simply not the same one.
    #[error("claim names bucket {named} but this consumer reads {expected}")]
    BucketMismatch { expected: ClaimBucket, named: ClaimBucket },
    /// The header carried a value that is not a legal bucket name, so there is
    /// nothing to compare against the bucket this consumer opened. Kept apart from a
    /// mismatch because it says something different: a publisher naming a real
    /// but foreign bucket is a deployment pointed the wrong way, whereas a name
    /// no NATS server would accept is a corrupted or forged header.
    #[error("claim names {named:?}, which is not a bucket name: {source}")]
    UnnamableBucket {
        named: ClaimBucketHeader,
        #[source]
        source: ClaimBucketError,
    },
    #[error("failed to resolve claim from object store: {0}")]
    StoreFailed(#[source] E),
    #[error("failed to read claim payload: {0}")]
    ReadFailed(#[from] std::io::Error),
}

/// The producing half of a claim check: a store to offload oversized bodies
/// into, and the bucket name every claim it publishes carries.
///
/// The two arrive as a [`ClaimBucketBinding`] for the same reason the consumer
/// takes one. Here the name is not checked but asserted, so a store and a name
/// passed separately could put the body in one bucket and send consumers to
/// another, which no test of either half on its own would catch.
#[derive(Clone)]
pub struct ClaimCheckPublisher<P, S> {
    publisher: P,
    store: S,
    bucket: ClaimBucket,
    max_payload: Arc<dyn MaxPayloadLimit>,
}

impl<P: fmt::Debug, S: fmt::Debug> fmt::Debug for ClaimCheckPublisher<P, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimCheckPublisher")
            .field("publisher", &self.publisher)
            .field("store", &self.store)
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

impl<P, S> ClaimCheckPublisher<P, S> {
    pub fn new<M: MaxPayloadLimit>(publisher: P, binding: ClaimBucketBinding<S>, max_payload: M) -> Self {
        let (store, bucket) = binding.into_parts();
        Self {
            publisher,
            store,
            bucket,
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
        // Cleanup relies on the object store bucket's retention, which is sized
        // from the owning stream via `ClaimRetention`.
        let mut cursor = std::io::Cursor::new(payload);
        if let Err(e) = self.store.put(&key, &mut cursor).await {
            error!(error = %e, "claim check: failed to store payload in object store");
            return PublishOutcome::StoreFailed(Box::new(e));
        }

        let mut claim_headers = headers;
        claim_headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        claim_headers.insert(HEADER_CLAIM_BUCKET, self.bucket.as_str());
        claim_headers.insert(HEADER_CLAIM_KEY, key.as_str());

        super::publish::publish_event(&self.publisher, subject, claim_headers, Bytes::new(), ack_timeout).await
    }
}

#[cfg(all(test, feature = "test-support"))]
mod integration_tests;
#[cfg(test)]
mod tests;
