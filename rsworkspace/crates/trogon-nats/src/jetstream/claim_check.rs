use std::fmt;
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

    let key = headers
        .get(HEADER_CLAIM_KEY)
        .ok_or(ClaimResolveError::MissingKey)?;

    let mut reader = store
        .get(key.as_str())
        .await
        .map_err(ClaimResolveError::StoreFailed)?;

    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .await
        .map_err(ClaimResolveError::ReadFailed)?;

    Ok(Bytes::from(buf))
}

#[derive(Debug)]
pub enum ClaimResolveError<E: fmt::Display> {
    MissingKey,
    StoreFailed(E),
    ReadFailed(std::io::Error),
}

impl<E: fmt::Display> fmt::Display for ClaimResolveError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKey => write!(f, "claim message missing {} header", HEADER_CLAIM_KEY),
            Self::StoreFailed(e) => write!(f, "failed to resolve claim from object store: {e}"),
            Self::ReadFailed(e) => write!(f, "failed to read claim payload: {e}"),
        }
    }
}

impl<E: std::error::Error + Send + Sync + 'static> std::error::Error for ClaimResolveError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingKey => None,
            Self::StoreFailed(e) => Some(e),
            Self::ReadFailed(e) => Some(e),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClaimCheckPublisher<P, S> {
    publisher: P,
    store: S,
    bucket_name: String,
    max_payload: MaxPayload,
}

impl<P, S> ClaimCheckPublisher<P, S> {
    pub fn new(publisher: P, store: S, bucket_name: String, max_payload: MaxPayload) -> Self {
        Self {
            publisher,
            store,
            bucket_name,
            max_payload,
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
        let threshold = self.max_payload.threshold();

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

        super::publish::publish_event(
            &self.publisher,
            subject,
            claim_headers,
            Bytes::new(),
            ack_timeout,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_payload_subtracts_overhead() {
        let mp = MaxPayload::from_server_limit(1_048_576);
        assert_eq!(mp.threshold(), 1_048_576 - PROTOCOL_OVERHEAD);
    }

    #[test]
    fn max_payload_saturates_at_zero() {
        let mp = MaxPayload::from_server_limit(100);
        assert_eq!(mp.threshold(), 0);
    }

    #[test]
    fn is_claim_returns_true_for_v1_header() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        assert!(is_claim(&headers));
    }

    #[test]
    fn is_claim_returns_false_for_missing_header() {
        let headers = HeaderMap::new();
        assert!(!is_claim(&headers));
    }

    #[test]
    fn is_claim_returns_false_for_wrong_version() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CLAIM_CHECK, "v99");
        assert!(!is_claim(&headers));
    }

    #[test]
    fn claim_object_key_contains_subject() {
        let key = claim_object_key("github.push");
        assert!(key.starts_with("github.push/"));
    }

    #[test]
    fn strip_claim_headers_removes_claim_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "keep");
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        headers.insert(HEADER_CLAIM_BUCKET, "bucket");
        headers.insert(HEADER_CLAIM_KEY, "key");

        let stripped = strip_claim_headers(headers);
        assert!(stripped.get(HEADER_CLAIM_CHECK).is_none());
        assert!(stripped.get(HEADER_CLAIM_BUCKET).is_none());
        assert!(stripped.get(HEADER_CLAIM_KEY).is_none());
        assert_eq!(stripped.get("X-Custom").unwrap().as_str(), "keep");
    }

    #[test]
    fn strip_claim_headers_noop_when_clean() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "value");

        let stripped = strip_claim_headers(headers);
        assert_eq!(stripped.get("X-Custom").unwrap().as_str(), "value");
    }

    #[test]
    fn claim_resolve_error_display_missing_key() {
        let err: ClaimResolveError<String> = ClaimResolveError::MissingKey;
        let msg = err.to_string();
        assert!(msg.contains(HEADER_CLAIM_KEY));
    }

    #[test]
    fn claim_resolve_error_display_store_failed() {
        let err: ClaimResolveError<String> =
            ClaimResolveError::StoreFailed("connection refused".to_string());
        let msg = err.to_string();
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn claim_resolve_error_display_read_failed() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err: ClaimResolveError<String> = ClaimResolveError::ReadFailed(io_err);
        let msg = err.to_string();
        assert!(msg.contains("pipe broke"));
    }

    #[test]
    fn claim_resolve_error_source_missing_key() {
        use std::error::Error;
        let err: ClaimResolveError<std::io::Error> = ClaimResolveError::MissingKey;
        assert!(err.source().is_none());
    }

    #[test]
    fn claim_resolve_error_source_store_failed() {
        use std::error::Error;
        let inner = std::io::Error::other("boom");
        let err: ClaimResolveError<std::io::Error> = ClaimResolveError::StoreFailed(inner);
        assert!(err.source().is_some());
    }

    #[test]
    fn claim_resolve_error_source_read_failed() {
        use std::error::Error;
        let inner = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err: ClaimResolveError<std::io::Error> = ClaimResolveError::ReadFailed(inner);
        assert!(err.source().is_some());
    }
}

#[cfg(all(test, feature = "test-support"))]
mod integration_tests {
    use std::time::Duration;

    use super::*;
    use crate::jetstream::mocks::MockJetStreamPublisher;
    use crate::jetstream::mocks::MockObjectStore;

    #[tokio::test]
    async fn small_payload_publishes_directly() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 512]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload.len(), 512);
        assert!(store.stored_objects().is_empty());
    }

    #[tokio::test]
    async fn large_payload_stores_in_object_store_and_publishes_claim() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 2048]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].payload.is_empty());
        assert_eq!(
            messages[0]
                .headers
                .get(HEADER_CLAIM_CHECK)
                .unwrap()
                .as_str(),
            CLAIM_CHECK_VERSION
        );
        assert_eq!(
            messages[0]
                .headers
                .get(HEADER_CLAIM_BUCKET)
                .unwrap()
                .as_str(),
            "test-bucket"
        );
        let key = messages[0]
            .headers
            .get(HEADER_CLAIM_KEY)
            .unwrap()
            .as_str()
            .to_string();
        assert!(key.starts_with("test.subject/"));

        let stored = store.stored_objects();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, key);
        assert_eq!(stored[0].1.len(), 2048);
    }

    #[tokio::test]
    async fn payload_at_exact_threshold_publishes_directly() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 1024]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        assert_eq!(publisher.published_messages()[0].payload.len(), 1024);
        assert!(store.stored_objects().is_empty());
    }

    #[tokio::test]
    async fn object_store_failure_returns_store_failed() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        store.fail_next_put();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store,
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 2048]),
                Duration::from_secs(5),
            )
            .await;

        assert!(!outcome.is_ok());
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn large_payload_preserves_original_headers() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store,
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "value");

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                headers,
                Bytes::from(vec![0u8; 2048]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        let msg = &publisher.published_messages()[0];
        assert_eq!(msg.headers.get("X-Custom").unwrap().as_str(), "value");
        assert!(msg.headers.get(HEADER_CLAIM_CHECK).is_some());
    }

    #[tokio::test]
    async fn resolve_claim_returns_payload_when_not_a_claim() {
        let store = MockObjectStore::new();
        let headers = HeaderMap::new();
        let payload = Bytes::from("raw data");

        let result = resolve_claim(&headers, payload.clone(), &store).await;
        assert_eq!(result.unwrap(), payload);
    }

    #[tokio::test]
    async fn resolve_claim_fetches_from_store() {
        let store = MockObjectStore::new();
        let expected = Bytes::from("large payload data");
        store.seed("test.subject/some-id", expected.clone());

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        headers.insert(HEADER_CLAIM_BUCKET, "test-bucket");
        headers.insert(HEADER_CLAIM_KEY, "test.subject/some-id");

        let result = resolve_claim(&headers, Bytes::new(), &store).await;
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn resolve_claim_errors_on_missing_key_header() {
        let store = MockObjectStore::new();
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);

        let result = resolve_claim(&headers, Bytes::new(), &store).await;
        assert!(matches!(result, Err(ClaimResolveError::MissingKey)));
    }

    #[tokio::test]
    async fn small_payload_strips_claim_headers() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "keep");
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        headers.insert(HEADER_CLAIM_BUCKET, "rogue-bucket");
        headers.insert(HEADER_CLAIM_KEY, "rogue-key");

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                headers,
                Bytes::from(vec![0u8; 512]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        let msg = &publisher.published_messages()[0];
        assert_eq!(msg.headers.get("X-Custom").unwrap().as_str(), "keep");
        assert!(msg.headers.get(HEADER_CLAIM_CHECK).is_none());
        assert!(msg.headers.get(HEADER_CLAIM_BUCKET).is_none());
        assert!(msg.headers.get(HEADER_CLAIM_KEY).is_none());
        assert!(store.stored_objects().is_empty());
    }
}
