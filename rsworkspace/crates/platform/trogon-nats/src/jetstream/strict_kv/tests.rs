use super::*;
use crate::{
    NatsToken,
    jetstream::{NativeKvScope, NativeStreamReadError},
    test_support::{CoreTestServer, JetStreamTestServer},
};
use async_nats::jetstream::{self, stream};
use bytes::Bytes;
use futures::StreamExt;

async fn fixture() -> (JetStreamTestServer, jetstream::Context, StrictNativeKvStore) {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_key_value(kv::Config {
            bucket: "STRICT_NATIVE_READ".into(),
            storage: stream::StorageType::File,
            history: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    let store = StrictNativeKvStore::new(
        context.clone(),
        NativeKvScope::new(NatsToken::new("STRICT_NATIVE_READ").unwrap()).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    (server, context, store)
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
async fn strict_native_kv_read_refuses_unknown_and_ambiguous_stored_operations() {
    let (_server, context, store) = fixture().await;
    let mut headers = HeaderMap::new();
    headers.insert("KV-Operation", "FUTURE_OPERATION");
    publish(&context, headers).await;
    assert!(matches!(
        store.read_entry("entry").await,
        Err(NativeKvReadError::UnknownOperation)
    ));
    let mut headers = HeaderMap::new();
    headers.append("KV-Operation", "PUT");
    headers.append("KV-Operation", "DEL");
    publish(&context, headers).await;
    assert!(matches!(
        JetStreamStrictKvRead::read_entry(&store, "entry").await,
        Err(NativeKvReadError::AmbiguousOperation)
    ));
    let mut headers = HeaderMap::new();
    headers.append("KV-Operation", "PUT");
    headers.append("KV-Operation", "PUT");
    publish(&context, headers).await;
    assert!(matches!(
        store.read_entry("entry").await,
        Err(NativeKvReadError::AmbiguousOperation)
    ));
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Marker-Reason", "FutureMarker");
    publish(&context, headers).await;
    assert!(matches!(
        store.read_entry("entry").await,
        Err(NativeKvReadError::UnknownMarker)
    ));
}
#[tokio::test]
async fn strict_native_kv_reads_preserve_native_put_revision_and_tombstones() {
    let (_server, context, store) = fixture().await;
    let revision = publish(&context, HeaderMap::new()).await;
    let entry = store.read_entry("entry").await.unwrap().unwrap();
    assert_eq!(entry.operation, kv::Operation::Put);
    assert_eq!(entry.revision, revision);
    assert_eq!(entry.value, Bytes::from_static(b"synthetic bounded value"));
    assert_eq!(entry.bucket, "STRICT_NATIVE_READ");
    assert_eq!(entry.key, "entry");
    store.bucket().delete("entry").await.unwrap();
    assert_eq!(
        store.read_entry("entry").await.unwrap().unwrap().operation,
        kv::Operation::Delete
    );
    store.bucket().purge("entry").await.unwrap();
    assert_eq!(
        store.read_entry("entry").await.unwrap().unwrap().operation,
        kv::Operation::Purge
    );
}
#[tokio::test]
async fn strict_native_kv_read_keeps_absence_invalid_key_and_record_limit_distinct() {
    let (_server, context, store) = fixture().await;
    assert!(store.read_entry("entry").await.unwrap().is_none());
    let invalid_key = "entry..child";
    let error = store.read_entry(invalid_key).await.unwrap_err();
    assert!(matches!(error, NativeKvReadError::InvalidKey));
    assert_eq!(format!("{error:?}"), "native KV read key is invalid");
    assert_eq!(format!("{error}"), "native KV read key is invalid");
    assert!(!format!("{error:?}").contains(invalid_key));
    context
        .publish("$KV.STRICT_NATIVE_READ.entry", vec![0; 1025].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let error = store.read_entry("entry").await.unwrap_err();
    assert!(matches!(error, NativeKvReadError::Read(NativeStreamReadError::Limit)));
    assert!(!format!("{error:?}").contains("1025"));
}
#[tokio::test]
async fn strict_native_kv_constructor_keeps_failed_lookup_and_invalid_profile_distinct() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let scope = NativeKvScope::new(NatsToken::new("missing").unwrap()).unwrap();
    let error = StrictNativeKvStore::new(
        context.clone(),
        scope.clone(),
        NonZeroUsize::new(1024).unwrap(),
        Duration::from_secs(2),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, NativeKvReadError::Lookup(_)));
    assert_eq!(format!("{error:?}"), "NativeKvReadError { .. }");
    assert!(matches!(
        StrictNativeKvStore::new(context, scope, NonZeroUsize::new(1024).unwrap(), Duration::ZERO).await,
        Err(NativeKvReadError::Read(NativeStreamReadError::InvalidBinding))
    ));
}

async fn capture_info(context: &jetstream::Context) -> Bytes {
    context
        .client()
        .request(
            "$JS.API.STREAM.INFO.KV_STRICT_NATIVE_READ",
            serde_json::to_vec(&()).unwrap().into(),
        )
        .await
        .unwrap()
        .payload
}

async fn supply_info(client: &async_nats::Client, info: Bytes) -> tokio::task::JoinHandle<()> {
    let mut requests = client
        .subscribe("$JS.API.STREAM.INFO.KV_STRICT_NATIVE_READ")
        .await
        .unwrap();
    client.flush().await.unwrap();
    let client = client.clone();
    tokio::spawn(async move {
        let request = requests.next().await.unwrap();
        client.publish(request.reply.unwrap(), info).await.unwrap();
        client.flush().await.unwrap();
    })
}
#[tokio::test]
async fn strict_native_kv_constructor_refuses_mismatched_native_stream_evidence() {
    let (_server, context, _) = fixture().await;
    let core = CoreTestServer::start().await;
    let client = async_nats::connect(core.address().to_owned()).await.unwrap();
    let mut info: serde_json::Value = serde_json::from_slice(&capture_info(&context).await).unwrap();
    assert_eq!(info["config"]["name"].as_str(), Some("KV_STRICT_NATIVE_READ"));
    info["config"]["name"] = "KV_OTHER".into();
    let task = supply_info(&client, serde_json::to_vec(&info).unwrap().into()).await;
    let result = StrictNativeKvStore::new(
        jetstream::new(client),
        NativeKvScope::new(NatsToken::new("STRICT_NATIVE_READ").unwrap()).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
        Duration::from_secs(2),
    )
    .await;
    assert!(matches!(result, Err(NativeKvReadError::InvalidBinding)));
    task.await.unwrap();
}
#[tokio::test]
async fn strict_native_kv_keeps_unavailable_protocol_reply_distinct_from_corrupt_record() {
    let (_server, context, _) = fixture().await;
    let core = CoreTestServer::start_with_config(
        r#"mappings: { "$JS.API.STREAM.MSG.GET.KV_STRICT_NATIVE_READ": "unavailable.native" }"#,
    )
    .await;
    let client = async_nats::connect(core.address().to_owned()).await.unwrap();
    let task = supply_info(&client, capture_info(&context).await).await;
    let store = StrictNativeKvStore::new(
        jetstream::new(client),
        NativeKvScope::new(NatsToken::new("STRICT_NATIVE_READ").unwrap()).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    task.await.unwrap();
    let error = store.read_entry("entry").await.unwrap_err();
    let NativeKvReadError::Read(source) = error else {
        panic!("unexpected native KV error class: {error:?}");
    };
    assert!(
        matches!(
            source,
            NativeStreamReadError::ProtocolStatus(async_nats::StatusCode::NO_RESPONDERS)
        ),
        "unexpected native stream failure: {source:?}"
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

#[test]
fn strict_native_kv_raw_parser_reports_the_stored_header_budget_separately() {
    assert!(matches!(
        strict_native_kv_raw_operation(&vec![b'x'; 16 * 1024 + 1]),
        Err(NativeKvReadError::RecordLimit)
    ));
}
