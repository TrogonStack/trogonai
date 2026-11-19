#![allow(clippy::unwrap_used)]
#![cfg(feature = "test-support")]

use async_nats::jetstream::{Context, kv};
use bytes::Bytes;
use std::{num::NonZeroUsize, time::Duration};
use trogon_nats::{
    NatsToken,
    jetstream::{
        NativeKvCasError, NativeKvExpectation, NativeKvKey, NativeKvScope, NativeKvUpdate, StrictNativeKvStore,
        StrictNativeStreamReader, conditional_native_kv_update,
    },
    test_support::JetStreamTestServer,
};

async fn create_bucket(context: &Context) -> kv::Store {
    context
        .create_key_value(kv::Config {
            bucket: "NATIVE_CONTEXT".into(),
            ..Default::default()
        })
        .await
        .unwrap()
}

fn request<'a>(scope: &'a NativeKvScope, key: &'a NativeKvKey) -> NativeKvUpdate<'a> {
    NativeKvUpdate::new(
        scope,
        key,
        Bytes::from_static(b"original bytes"),
        NativeKvExpectation::from_sequences(0, 0).unwrap(),
        NonZeroUsize::new(8192).unwrap(),
    )
}

#[tokio::test]
async fn conditional_publication_stays_in_its_own_native_context() {
    let original_server = JetStreamTestServer::start().await;
    let foreign_server = JetStreamTestServer::start().await;
    let original_context = original_server.jetstream().await;
    let foreign_context = foreign_server.jetstream().await;
    let original_bucket = create_bucket(&original_context).await;
    let foreign_bucket = create_bucket(&foreign_context).await;
    let scope = NativeKvScope::new(NatsToken::new("NATIVE_CONTEXT").unwrap()).unwrap();
    let key = NativeKvKey::new("entry").unwrap();
    let revision = conditional_native_kv_update(&original_context, request(&scope, &key))
        .await
        .unwrap();
    assert_eq!(
        original_bucket.entry("entry").await.unwrap().unwrap().revision,
        revision.get()
    );
    assert!(foreign_bucket.entry("entry").await.unwrap().is_none());
}

#[tokio::test]
async fn missing_native_bucket_is_a_predispatch_lookup_failure() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let existing_bucket = create_bucket(&context).await;
    let missing = NativeKvScope::new(NatsToken::new("MISSING_NATIVE_BUCKET").unwrap()).unwrap();
    let key = NativeKvKey::new("entry").unwrap();
    assert!(matches!(
        conditional_native_kv_update(&context, request(&missing, &key)).await,
        Err(NativeKvCasError::Lookup(_))
    ));
    assert!(existing_bucket.entry("entry").await.unwrap().is_none());
}

#[tokio::test]
async fn acknowledged_long_native_keys_roundtrip_through_all_scoped_readers() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let bucket_name = "b".repeat(125);
    context
        .create_key_value(kv::Config {
            bucket: bucket_name.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    let scope = NativeKvScope::new(NatsToken::new(&bucket_name).unwrap()).unwrap();
    let maximum_record_bytes = NonZeroUsize::new(8192).unwrap();
    let timeout = Duration::from_secs(5);
    let reader = StrictNativeStreamReader::new(context.clone(), maximum_record_bytes, timeout).unwrap();
    let strict_store = StrictNativeKvStore::new(context.clone(), scope.clone(), maximum_record_bytes, timeout)
        .await
        .unwrap();
    let mut expected_global = 0;
    for key_length in [300, 1024] {
        let key = NativeKvKey::new("k".repeat(key_length)).unwrap();
        let value = Bytes::from(format!("original value for native key length {key_length}"));
        let revision = conditional_native_kv_update(
            &context,
            NativeKvUpdate::new(
                &scope,
                &key,
                value.clone(),
                NativeKvExpectation::from_sequences(0, expected_global).unwrap(),
                maximum_record_bytes,
            ),
        )
        .await
        .unwrap();
        assert_eq!(revision.get(), expected_global + 1);
        let by_sequence = reader.read_kv_sequence(&scope, &key, revision).await.unwrap().unwrap();
        assert_eq!(by_sequence.sequence, revision.get());
        assert_eq!(by_sequence.subject.as_str(), scope.subject(&key));
        assert_eq!(by_sequence.payload, value);
        let last = reader.read_kv_last(&scope, &key).await.unwrap().unwrap();
        assert_eq!(last.sequence, revision.get());
        assert_eq!(last.subject.as_str(), scope.subject(&key));
        assert_eq!(last.payload, value);
        let entry = strict_store.read_entry(key.as_str()).await.unwrap().unwrap();
        assert_eq!(entry.revision, revision.get());
        assert_eq!(entry.bucket, bucket_name);
        assert_eq!(entry.key, key.as_str());
        assert_eq!(entry.value, value);
        assert_eq!(entry.operation, kv::Operation::Put);
        expected_global = revision.get();
    }
}
