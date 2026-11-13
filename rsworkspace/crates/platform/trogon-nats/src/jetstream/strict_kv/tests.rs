use super::*;
use crate::{
    jetstream::{JetStreamKvEntry, JetStreamKvGet},
    test_support::JetStreamTestServer,
};
use async_nats::jetstream::{self, stream};
use bytes::Bytes;
use std::error::Error;

async fn fixture() -> (JetStreamTestServer, jetstream::Context, StrictNativeKvStore) {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let bucket = context
        .create_key_value(kv::Config {
            bucket: "STRICT_NATIVE_READ".into(),
            storage: stream::StorageType::File,
            history: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    (
        server,
        context.clone(),
        StrictNativeKvStore::new(
            context,
            bucket,
            NonZeroUsize::new(1024).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap(),
    )
}
async fn publish(context: &jetstream::Context, headers: HeaderMap) -> u64 {
    context
        .publish_with_headers(
            "$KV.STRICT_NATIVE_READ.entry",
            headers,
            Bytes::from_static(b"synthetic bounded value"),
        )
        .await
        .unwrap()
        .await
        .unwrap()
        .sequence
}
#[tokio::test]
async fn strict_native_kv_entry_refuses_unknown_operations_from_actual_broker() {
    let (_server, context, bucket) = fixture().await;
    let mut headers = HeaderMap::new();
    headers.insert("KV-Operation", "FUTURE_OPERATION");
    publish(&context, headers).await;
    assert!(
        <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn strict_native_kv_get_refuses_unknown_operations_from_actual_broker() {
    let (_server, context, bucket) = fixture().await;
    let mut headers = HeaderMap::new();
    headers.insert("KV-Operation", "FUTURE_OPERATION");
    publish(&context, headers).await;
    assert!(
        <StrictNativeKvStore as JetStreamKvGet>::get(&bucket, "entry".into())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn strict_native_kv_entry_refuses_duplicate_operations_from_actual_broker() {
    let (_server, context, bucket) = fixture().await;
    let mut headers = HeaderMap::new();
    headers.append("KV-Operation", "PUT");
    headers.append("KV-Operation", "DEL");
    publish(&context, headers).await;
    let error = <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
        .await
        .unwrap_err();
    assert!(matches!(
        error
            .source()
            .and_then(|source| source.downcast_ref::<NativeKvReadError>()),
        Some(NativeKvReadError::AmbiguousOperation)
    ));
}
#[tokio::test]
async fn strict_native_kv_get_refuses_duplicate_operations_from_actual_broker() {
    let (_server, context, bucket) = fixture().await;
    let mut headers = HeaderMap::new();
    headers.append("KV-Operation", "PUT");
    headers.append("KV-Operation", "PUT");
    publish(&context, headers).await;
    let error = <StrictNativeKvStore as JetStreamKvGet>::get(&bucket, "entry".into())
        .await
        .unwrap_err();
    assert!(matches!(
        error
            .source()
            .and_then(|source| source.downcast_ref::<NativeKvReadError>()),
        Some(NativeKvReadError::AmbiguousOperation)
    ));
}
#[tokio::test]
async fn strict_native_kv_reads_preserve_native_put_revision_and_tombstones() {
    let (_server, context, bucket) = fixture().await;
    let revision = publish(&context, HeaderMap::new()).await;
    let entry = <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.operation, kv::Operation::Put);
    assert_eq!(entry.revision, revision);
    assert_eq!(
        <StrictNativeKvStore as JetStreamKvGet>::get(&bucket, "entry".into())
            .await
            .unwrap(),
        Some(Bytes::from_static(b"synthetic bounded value"))
    );
    bucket.bucket().delete("entry").await.unwrap();
    assert_eq!(
        <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
            .await
            .unwrap()
            .unwrap()
            .operation,
        kv::Operation::Delete
    );
    assert!(
        <StrictNativeKvStore as JetStreamKvGet>::get(&bucket, "entry".into())
            .await
            .unwrap()
            .is_none()
    );
    bucket.bucket().purge("entry").await.unwrap();
    assert_eq!(
        <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
            .await
            .unwrap()
            .unwrap()
            .operation,
        kv::Operation::Purge
    );
    assert!(
        <StrictNativeKvStore as JetStreamKvGet>::get(&bucket, "entry".into())
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn strict_native_kv_entry_refuses_unknown_marker_reason_from_actual_broker() {
    let (_server, context, bucket) = fixture().await;
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Marker-Reason", "FutureMarker");
    publish(&context, headers).await;
    assert!(
        <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
            .await
            .is_err()
    );
}
#[test]
fn strict_native_kv_operation_parser_never_defaults_ambiguous_or_unknown_semantics_to_put() {
    for (name, value) in [
        ("KV-Operation", "FUTURE_OPERATION"),
        ("KV-Operation", ""),
        ("Nats-Marker-Reason", "FutureMarker"),
        ("kv-operation", "DEL"),
        ("KV-Future", "PUT"),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(name, value);
        assert!(
            strict_native_kv_operation(Some(&headers)).is_err(),
            "accepted ambiguous operation metadata"
        );
    }
    let mut duplicate = HeaderMap::new();
    duplicate.append("KV-Operation", "PUT");
    duplicate.append("KV-Operation", "PUT");
    assert!(strict_native_kv_operation(Some(&duplicate)).is_err());
    let mut mixed = HeaderMap::new();
    mixed.insert("KV-Operation", "PUT");
    mixed.insert("Nats-Marker-Reason", "Remove");
    assert!(strict_native_kv_operation(Some(&mixed)).is_err());
}
#[test]
fn strict_native_kv_operation_parser_preserves_known_operation_and_marker_semantics() {
    assert_eq!(strict_native_kv_operation(None).unwrap(), kv::Operation::Put);
    for (name, value, expected) in [
        ("KV-Operation", "PUT", kv::Operation::Put),
        ("KV-Operation", "DEL", kv::Operation::Delete),
        ("KV-Operation", "PURGE", kv::Operation::Purge),
        ("Nats-Marker-Reason", "MaxAge", kv::Operation::Purge),
        ("Nats-Marker-Reason", "Purge", kv::Operation::Purge),
        ("Nats-Marker-Reason", "Remove", kv::Operation::Delete),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(name, value);
        assert_eq!(strict_native_kv_operation(Some(&headers)).unwrap(), expected);
    }
}

#[tokio::test]
async fn strict_native_kv_read_keeps_absence_invalid_key_and_record_limit_distinct() {
    let (_server, context, bucket) = fixture().await;
    assert!(
        <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry..child".into())
            .await
            .unwrap_err()
            .kind(),
        kv::EntryErrorKind::InvalidKey
    );
    context
        .publish("$KV.STRICT_NATIVE_READ.entry", vec![0; 1025].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let error = <StrictNativeKvStore as JetStreamKvEntry>::entry(&bucket, "entry".into())
        .await
        .unwrap_err();
    assert!(matches!(
        error
            .source()
            .and_then(|source| source.downcast_ref::<NativeKvReadError>()),
        Some(NativeKvReadError::RecordLimit)
    ));
}
#[tokio::test]
async fn strict_native_kv_wrapper_refuses_redirected_and_cross_bucket_handles() {
    let (_server, context, bucket) = fixture().await;
    let mut invalid = bucket.bucket().clone();
    invalid.prefix = "$KV.OTHER.".into();
    assert!(matches!(
        StrictNativeKvStore::new(
            context.clone(),
            invalid,
            NonZeroUsize::new(1024).unwrap(),
            Duration::from_secs(5)
        ),
        Err(NativeKvReadError::InvalidBinding)
    ));
    let mut invalid = bucket.bucket().clone();
    invalid.put_prefix = Some("$KV.OTHER.".into());
    assert!(matches!(
        StrictNativeKvStore::new(
            context,
            invalid,
            NonZeroUsize::new(1024).unwrap(),
            Duration::from_secs(5)
        ),
        Err(NativeKvReadError::InvalidBinding)
    ));
}
#[test]
fn strict_native_kv_raw_parser_preserves_duplicate_cardinality_and_rejects_folding() {
    for raw in [
        b"NATS/1.0\r\nKV-Operation: PUT\r\nKV-Operation: PUT\r\n\r\n".as_slice(),
        b"NATS/1.0\r\nKV-Operation: PUT\r\nkv-operation: PUT\r\n\r\n",
        b"NATS/1.0\r\nNats-Marker-Reason: Remove\r\nNats-Marker-Reason: Remove\r\n\r\n",
    ] {
        assert!(matches!(
            strict_native_kv_raw_operation(raw),
            Err(NativeKvReadError::AmbiguousOperation)
        ));
    }
    for raw in [
        b"NATS/1.0\r\nKV-Operation: PUT\r\n continuation\r\n\r\n".as_slice(),
        b"NATS/1.0\nKV-Operation: PUT\n\n",
        b"NATS/1.0 200 OK\r\n\r\n",
        b"NATS/1.0\r\nKV-Operation: PUT\r\n",
    ] {
        assert!(matches!(
            strict_native_kv_raw_operation(raw),
            Err(NativeKvReadError::InvalidRecord)
        ));
    }
    assert_eq!(
        strict_native_kv_raw_operation(b"NATS/1.0\r\n\r\n").unwrap(),
        kv::Operation::Put
    );
    assert_eq!(
        strict_native_kv_raw_operation(b"NATS/1.0\r\nNats-Marker-Reason: MaxAge\r\n\r\n").unwrap(),
        kv::Operation::Purge
    );
    assert_eq!(
        strict_native_kv_raw_operation(b"NATS/1.0\r\nKV-Operation: DEL\r\n\r\n").unwrap(),
        kv::Operation::Delete
    );
}
