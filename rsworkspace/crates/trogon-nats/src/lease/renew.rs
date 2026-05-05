use std::future::IntoFuture;
use std::time::Duration;

use async_nats::header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_TTL};
use async_nats::jetstream::kv;
use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;

use crate::jetstream::{JetStreamKeyValueUpdate, JetStreamPublisher};

use super::{NatsKvLease, RenewLease};

#[derive(Clone)]
#[cfg_attr(coverage, allow(dead_code))]
pub(super) struct KvPublishTarget {
    default_prefix: String,
    put_prefix: Option<String>,
    uses_jetstream_prefix: bool,
}

#[cfg_attr(coverage, allow(dead_code))]
impl KvPublishTarget {
    pub(super) fn from_store(store: &kv::Store) -> Self {
        Self {
            default_prefix: store.prefix.clone(),
            put_prefix: store.put_prefix.clone(),
            uses_jetstream_prefix: store.use_jetstream_prefix,
        }
    }

    #[cfg(test)]
    fn new(
        default_prefix: impl Into<String>,
        put_prefix: Option<impl Into<String>>,
        uses_jetstream_prefix: bool,
    ) -> Self {
        Self {
            default_prefix: default_prefix.into(),
            put_prefix: put_prefix.map(Into::into),
            uses_jetstream_prefix,
        }
    }
}

impl RenewLease for NatsKvLease {
    type Error = kv::UpdateError;

    async fn renew(&self, value: Bytes, revision: u64) -> Result<u64, Self::Error> {
        renew_store(
            &self.store,
            &self.publish_target,
            self.js_context.as_ref(),
            self.key.as_str(),
            self.ttl,
            value,
            revision,
        )
        .await
    }
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) async fn renew_store<S, P>(
    store: &S,
    publish_target: &KvPublishTarget,
    publisher: Option<&P>,
    key: &str,
    ttl: Option<Duration>,
    value: Bytes,
    revision: u64,
) -> Result<u64, kv::UpdateError>
where
    S: JetStreamKeyValueUpdate,
    P: JetStreamPublisher,
{
    let Some(ttl) = ttl else {
        return store.update(key, value, revision).await;
    };
    let Some(publisher) = publisher else {
        return store.update(key, value, revision).await;
    };

    if publish_target.uses_jetstream_prefix {
        // TODO(yordis): support per-message TTL renew when KV store uses a custom
        // JetStream API prefix by routing through an SDK path that handles prefixing.
        let source = std::io::Error::other("per-message lease renew does not support custom JS API prefixes");
        return Err(kv::UpdateError::with_source(kv::UpdateErrorKind::Other, source));
    }

    let subject = build_kv_subject(
        publish_target.put_prefix.as_deref(),
        &publish_target.default_prefix,
        key,
    );
    let headers = build_update_headers(revision, ttl);

    let publish = publisher
        .publish_with_headers(subject, headers, value)
        .await
        .map_err(update_error_from_publish)?;
    let ack = IntoFuture::into_future(publish)
        .await
        .map_err(update_error_from_publish)?;
    Ok(ack.sequence)
}

#[cfg_attr(coverage, allow(dead_code))]
fn update_error_from_publish<E>(source: E) -> kv::UpdateError
where
    E: std::error::Error + Send + Sync + 'static,
{
    kv::UpdateError::with_source(kv::UpdateErrorKind::Other, source)
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn build_kv_subject(put_prefix: Option<&str>, default_prefix: &str, key: &str) -> String {
    let mut subject = String::new();
    subject.push_str(put_prefix.unwrap_or(default_prefix));
    subject.push_str(key);
    subject
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn build_update_headers(revision: u64, ttl: Duration) -> HeaderMap {
    let mut headers = HeaderMap::default();
    headers.insert(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, HeaderValue::from(revision));
    headers.insert(NATS_MESSAGE_TTL, HeaderValue::from(ttl.as_secs()));
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jetstream::{JetStreamPublisher, MockJetStreamKvStore, MockJetStreamPublisher};

    fn publish_target() -> KvPublishTarget {
        KvPublishTarget::new("$KV.bucket.", None::<String>, false)
    }

    #[tokio::test]
    async fn renew_uses_update_when_ttl_is_absent() {
        let store = MockJetStreamKvStore::new();
        let revision = renew_store::<_, MockJetStreamPublisher>(
            &store,
            &publish_target(),
            None,
            "key",
            None,
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap();

        assert_eq!(revision, 1);
        assert_eq!(
            store.update_calls(),
            vec![("key".to_string(), Bytes::from_static(b"value"), 7)]
        );
    }

    #[tokio::test]
    async fn renew_uses_update_when_publisher_is_missing() {
        let store = MockJetStreamKvStore::new();
        let revision = renew_store::<_, MockJetStreamPublisher>(
            &store,
            &publish_target(),
            None,
            "key",
            Some(Duration::from_secs(10)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap();

        assert_eq!(revision, 1);
        assert_eq!(
            store.update_calls(),
            vec![("key".to_string(), Bytes::from_static(b"value"), 7)]
        );
    }

    #[tokio::test]
    async fn renew_rejects_custom_jetstream_prefix() {
        let store = MockJetStreamKvStore::new();
        let publish_target = KvPublishTarget::new("$KV.bucket.", None::<String>, true);
        let publisher = MockJetStreamPublisher::new();

        let error = renew_store(
            &store,
            &publish_target,
            Some(&publisher),
            "key",
            Some(Duration::from_secs(10)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap_err();

        assert_eq!(error.kind(), kv::UpdateErrorKind::Other);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn renew_publishes_to_put_prefix_when_present() {
        let store = MockJetStreamKvStore::new();
        let publish_target = KvPublishTarget::new("$KV.bucket.", Some("$KV.override."), false);
        let publisher = MockJetStreamPublisher::new();

        let revision = renew_store(
            &store,
            &publish_target,
            Some(&publisher),
            "key",
            Some(Duration::from_secs(10)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap();

        assert_eq!(revision, 1);
        assert_eq!(publisher.published_subjects(), vec!["$KV.override.key"]);
    }

    #[tokio::test]
    async fn renew_uses_default_prefix_when_put_prefix_is_missing() {
        let store = MockJetStreamKvStore::new();
        let publish_target = publish_target();
        let publisher = MockJetStreamPublisher::new();

        let revision = renew_store(
            &store,
            &publish_target,
            Some(&publisher),
            "key",
            Some(Duration::from_secs(10)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap();

        assert_eq!(revision, 1);
        assert_eq!(publisher.published_subjects(), vec!["$KV.bucket.key"]);
    }

    #[tokio::test]
    async fn renew_sets_expected_revision_and_ttl_seconds_headers() {
        let store = MockJetStreamKvStore::new();
        let publish_target = publish_target();
        let publisher = MockJetStreamPublisher::new();

        renew_store(
            &store,
            &publish_target,
            Some(&publisher),
            "key",
            Some(Duration::from_secs(9)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap();

        let mut messages = publisher.published_messages();
        let message = messages.pop().unwrap();
        assert_eq!(
            message
                .headers
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            Some("7")
        );
        assert_eq!(
            message.headers.get(NATS_MESSAGE_TTL).map(|value| value.as_str()),
            Some("9")
        );
    }

    #[tokio::test]
    async fn renew_propagates_publish_errors() {
        let store = MockJetStreamKvStore::new();
        let publish_target = publish_target();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();

        let error = renew_store(
            &store,
            &publish_target,
            Some(&publisher),
            "key",
            Some(Duration::from_secs(10)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap_err();

        assert_eq!(error.kind(), kv::UpdateErrorKind::Other);
    }

    #[tokio::test]
    async fn renew_returns_publish_ack_sequence() {
        let store = MockJetStreamKvStore::new();
        let publish_target = publish_target();
        let publisher = MockJetStreamPublisher::new();

        let ack1 = JetStreamPublisher::publish_with_headers(&publisher, "preseed", HeaderMap::default(), Bytes::new())
            .await
            .unwrap()
            .into_future()
            .await
            .unwrap();
        assert_eq!(ack1.sequence, 1);

        let revision = renew_store(
            &store,
            &publish_target,
            Some(&publisher),
            "key",
            Some(Duration::from_secs(10)),
            Bytes::from_static(b"value"),
            7,
        )
        .await
        .unwrap();

        assert_eq!(revision, 2);
    }
}
