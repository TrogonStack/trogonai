use super::*;
use crate::{
    NatsToken,
    jetstream::{NativeKvExpectation, NativeKvKey, NativeKvScope},
    test_support::JetStreamTestServer,
};
use async_nats::jetstream::{
    Context,
    context::{PublishError, PublishErrorKind},
    message::OutboundMessage,
    publish::PublishAck,
    stream,
};
use bytes::Bytes;
use std::num::NonZeroUsize;
use std::{
    future::{IntoFuture, Ready, ready},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Fixture {
    server: JetStreamTestServer,
    bucket: kv::Store,
    scope: NativeKvScope,
}
impl Fixture {
    async fn new() -> Self {
        let server = JetStreamTestServer::start().await;
        let context = server.jetstream().await;
        let bucket = context
            .create_key_value(kv::Config {
                bucket: "CONDITIONAL_KV".into(),
                storage: stream::StorageType::File,
                history: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        let scope = NativeKvScope::new(NatsToken::new("CONDITIONAL_KV").unwrap()).unwrap();
        Self { server, bucket, scope }
    }
    async fn tail(&self) -> u64 {
        self.bucket.stream.get_info().await.unwrap().state.last_sequence
    }
}
fn request<'a>(
    scope: &'a NativeKvScope,
    key: &'a NativeKvKey,
    expected_key: u64,
    expected_global: u64,
) -> NativeKvUpdate<'a> {
    NativeKvUpdate::new(
        scope,
        key,
        Bytes::from_static(b"synthetic bounded value"),
        NativeKvExpectation::from_sequences(expected_key, expected_global).unwrap(),
        NonZeroUsize::new(8192).unwrap(),
    )
}

#[tokio::test]
async fn conditional_kv_updates_require_both_exact_native_sequences() {
    let f = Fixture::new().await;
    let context = f.server.jetstream().await;
    let key = NativeKvKey::new("entry").unwrap();
    let first = conditional_native_kv_update(&context, request(&f.scope, &key, 0, 0))
        .await
        .unwrap();
    assert_eq!(first.get(), 1);
    assert!(matches!(
        conditional_native_kv_update(&context, request(&f.scope, &key, 0, first.get())).await,
        Err(NativeKvCasError::Conflict)
    ));
    let next = conditional_native_kv_update(&context, request(&f.scope, &key, first.get(), first.get()))
        .await
        .unwrap();
    assert_eq!(next.get(), 2);
    assert_eq!(f.bucket.entry("entry").await.unwrap().unwrap().revision, next.get());
}

#[tokio::test]
async fn conditional_kv_same_snapshot_race_has_one_native_winner_for_different_keys() {
    let f = Fixture::new().await;
    let first_context = f.server.jetstream().await;
    let second_context = f.server.jetstream().await;
    let tail = f.tail().await;
    let a = NativeKvKey::new("first").unwrap();
    let b = NativeKvKey::new("second").unwrap();
    let (a, b) = tokio::join!(
        conditional_native_kv_update(&first_context, request(&f.scope, &a, 0, tail)),
        conditional_native_kv_update(&second_context, request(&f.scope, &b, 0, tail))
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
    assert!(matches!(
        if a.is_err() { a } else { b },
        Err(NativeKvCasError::Conflict)
    ));
    assert_eq!(f.bucket.stream.get_info().await.unwrap().state.messages, 1);
}

#[tokio::test]
async fn conditional_kv_strict_zero_never_resurrects_deleted_or_purged_keys() {
    let f = Fixture::new().await;
    let context = f.server.jetstream().await;
    for (key, purge) in [("deleted", false), ("purged", true)] {
        f.bucket.update(key, Bytes::from_static(b"original"), 0).await.unwrap();
        if purge {
            f.bucket.purge(key).await.unwrap();
        } else {
            f.bucket.delete(key).await.unwrap();
        }
        let marker = f.bucket.entry(key).await.unwrap().unwrap();
        assert_ne!(marker.operation, kv::Operation::Put);
        let typed = NativeKvKey::new(key).unwrap();
        assert!(matches!(
            conditional_native_kv_update(&context, request(&f.scope, &typed, 0, f.tail().await)).await,
            Err(NativeKvCasError::Conflict)
        ));
        let after = f.bucket.entry(key).await.unwrap().unwrap();
        assert_eq!(after.revision, marker.revision);
        assert_eq!(after.operation, marker.operation);
    }
}

#[derive(Clone)]
struct LostAck {
    context: Context,
    lose: Arc<AtomicBool>,
}
impl JetStreamPublishMessage for LostAck {
    type PublishError = PublishError;
    type AckFuture = Ready<Result<PublishAck, PublishError>>;
    async fn publish_message(&self, message: OutboundMessage) -> Result<Self::AckFuture, PublishError> {
        let ack = self.context.publish_message(message).await?.into_future().await?;
        Ok(if self.lose.swap(false, Ordering::SeqCst) {
            ready(Err(PublishError::new(PublishErrorKind::Other)))
        } else {
            ready(Ok(ack))
        })
    }
}
#[tokio::test]
async fn conditional_kv_lost_ack_retains_uncertainty_and_never_returns_a_revision() {
    let f = Fixture::new().await;
    let context = f.server.jetstream().await;
    let publisher = LostAck {
        context: context.clone(),
        lose: Arc::new(AtomicBool::new(true)),
    };
    let key = NativeKvKey::new("entry").unwrap();
    assert!(matches!(
        publish_checked_update(&f.bucket, &publisher, request(&f.scope, &key, 0, 0)).await,
        Err(NativeKvCasError::Uncertain {
            stage: NativeKvCasStage::Acknowledge,
            ..
        })
    ));
    assert!(f.bucket.entry("entry").await.unwrap().is_some());
    assert!(matches!(
        conditional_native_kv_update(&context, request(&f.scope, &key, 0, 0)).await,
        Err(NativeKvCasError::Conflict)
    ));
}

#[tokio::test]
async fn conditional_kv_envelope_bound_refuses_before_publishing() {
    let f = Fixture::new().await;
    let context = f.server.jetstream().await;
    let key = NativeKvKey::new("entry").unwrap();
    let request = NativeKvUpdate::new(
        &f.scope,
        &key,
        Bytes::from_static(b"value"),
        NativeKvExpectation::from_sequences(0, 0).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    assert!(matches!(
        conditional_native_kv_update(&context, request).await,
        Err(NativeKvCasError::EnvelopeLimit)
    ));
    assert_eq!(f.tail().await, 0);
}

#[test]
fn conditional_kv_namespace_and_sequence_aliases_cannot_be_constructed() {
    for value in ["", ">", "a.*", "a..b", "\0", ".a", "a."] {
        assert!(NativeKvKey::new(value).is_err());
    }
    assert!(NativeKvExpectation::from_sequences(2, 1).is_err());
    assert!(NativeKvExpectation::from_sequences(0, u64::MAX).is_err());
}

#[derive(Clone, Copy)]
enum Corruption {
    Zero,
    Sequence,
    Stream,
    Duplicate,
}
#[derive(Clone)]
struct CorruptAck {
    context: Context,
    corruption: Corruption,
}
impl JetStreamPublishMessage for CorruptAck {
    type PublishError = PublishError;
    type AckFuture = Ready<Result<PublishAck, PublishError>>;
    async fn publish_message(&self, message: OutboundMessage) -> Result<Self::AckFuture, PublishError> {
        let mut ack = self.context.publish_message(message).await?.into_future().await?;
        match self.corruption {
            Corruption::Zero => ack.sequence = 0,
            Corruption::Sequence => ack.sequence += 1,
            Corruption::Stream => ack.stream = "ANOTHER_STREAM".into(),
            Corruption::Duplicate => ack.duplicate = true,
        }
        Ok(ready(Ok(ack)))
    }
}
#[tokio::test]
async fn conditional_kv_inconsistent_ack_never_proves_the_exact_write() {
    let f = Fixture::new().await;
    let context = f.server.jetstream().await;
    for (index, corruption) in [
        Corruption::Zero,
        Corruption::Sequence,
        Corruption::Stream,
        Corruption::Duplicate,
    ]
    .into_iter()
    .enumerate()
    {
        let publisher = CorruptAck {
            context: context.clone(),
            corruption,
        };
        let key = NativeKvKey::new(format!("entry-{index}")).unwrap();
        assert!(matches!(
            publish_checked_update(&f.bucket, &publisher, request(&f.scope, &key, 0, f.tail().await)).await,
            Err(NativeKvCasError::InvalidAcknowledgement)
        ));
        assert!(f.bucket.entry(key.as_str()).await.unwrap().is_some());
    }
}
#[tokio::test]
async fn conditional_kv_refuses_redirected_bucket_handles_before_any_write() {
    let f = Fixture::new().await;
    let context = f.server.jetstream().await;
    let key = NativeKvKey::new("entry").unwrap();
    let mut redirected = f.bucket.clone();
    redirected.put_prefix = Some("$KV.ANOTHER.".into());
    assert!(matches!(
        publish_checked_update(&redirected, &context, request(&f.scope, &key, 0, 0)).await,
        Err(NativeKvCasError::InvalidBinding)
    ));
    assert_eq!(f.tail().await, 0);
}

#[derive(Clone)]
struct FailedPublish;

impl JetStreamPublishMessage for FailedPublish {
    type PublishError = PublishError;
    type AckFuture = Ready<Result<PublishAck, PublishError>>;

    async fn publish_message(&self, _message: OutboundMessage) -> Result<Self::AckFuture, PublishError> {
        Err(PublishError::new(PublishErrorKind::Other))
    }
}

#[tokio::test]
async fn conditional_kv_publish_failure_cannot_return_a_revision() {
    let f = Fixture::new().await;
    let key = NativeKvKey::new("entry").unwrap();
    assert!(matches!(
        publish_checked_update(&f.bucket, &FailedPublish, request(&f.scope, &key, 0, 0)).await,
        Err(NativeKvCasError::Uncertain {
            stage: NativeKvCasStage::Publish,
            ..
        })
    ));
    assert_eq!(f.tail().await, 0);
}

#[test]
fn conditional_envelope_refuses_headerless_publications() {
    let message = PublishMessage::build()
        .payload(Bytes::from_static(b"unconditional value"))
        .outbound_message("$KV.CONDITIONAL_KV.entry");
    assert!(envelope_bytes(&message).is_none());
}
