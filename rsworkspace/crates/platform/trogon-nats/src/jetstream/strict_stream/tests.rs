use super::*;
use crate::test_support::JetStreamTestServer;
use async_nats::{HeaderMap, jetstream::stream};

#[tokio::test]
async fn native_stream_reader_preserves_duplicate_raw_event_headers_and_exact_selection() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_stream(stream::Config {
            name: "RAW_EVENTS".into(),
            subjects: vec!["events.>".into()],
            storage: stream::StorageType::File,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.append("Trogon-Event-Type", "accepted");
    headers.append("Trogon-Event-Type", "denied");
    let first = context
        .publish_with_headers("events.one", headers, vec![1, 2, 3].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let last = context
        .publish("events.one", vec![4, 5].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let reader =
        StrictNativeStreamReader::new(context, NonZeroUsize::new(32).unwrap(), Duration::from_secs(2)).unwrap();
    let stream = NatsToken::new("RAW_EVENTS").unwrap();
    let observed = reader
        .read_sequence(&stream, NonZeroU64::new(first.sequence).unwrap())
        .await
        .unwrap()
        .expect("the exact stored event");
    assert_eq!(observed.sequence, first.sequence);
    assert_eq!(observed.payload.as_ref(), &[1, 2, 3]);
    let values = observed
        .headers
        .iter()
        .filter(|(name, _)| AsRef::<str>::as_ref(name) == "Trogon-Event-Type")
        .flat_map(|(_, values)| values.iter())
        .map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        vec!["accepted", "denied"],
        "duplicate authority metadata survives until the owning validator refuses it"
    );
    assert_eq!(
        reader
            .read_first(&stream, "events.one")
            .await
            .unwrap()
            .unwrap()
            .sequence,
        first.sequence
    );
    assert_eq!(
        reader.read_last(&stream, "events.one").await.unwrap().unwrap().sequence,
        last.sequence
    );
    assert!(reader.read_last(&stream, "events.none").await.unwrap().is_none());
    assert!(matches!(
        reader.read_last(&stream, "events.*").await,
        Err(NativeStreamReadError::InvalidBinding)
    ));
}

#[tokio::test]
async fn native_stream_reader_rejects_oversized_stored_payloads_before_returning_data() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_stream(stream::Config {
            name: "RAW_EVENTS".into(),
            subjects: vec!["events.>".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let ack = context
        .publish("events.one", vec![7; 64].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let reader =
        StrictNativeStreamReader::new(context, NonZeroUsize::new(32).unwrap(), Duration::from_secs(2)).unwrap();
    assert!(matches!(
        reader
            .read_sequence(
                &NatsToken::new("RAW_EVENTS").unwrap(),
                NonZeroU64::new(ack.sequence).unwrap()
            )
            .await,
        Err(NativeStreamReadError::Limit)
    ));
}

#[tokio::test]
async fn scoped_native_kv_sequence_preserves_original_headers_without_broadening_generic_reads() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_stream(stream::Config {
            name: "KV_scoped".into(),
            subjects: vec!["$KV.scoped.>".into()],
            storage: stream::StorageType::File,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.append("Trogon-Registration-Approval", "first");
    headers.append("Trogon-Registration-Approval", "second");
    let ack = context
        .publish_with_headers("$KV.scoped.exact", headers, vec![1, 2, 3].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let reader =
        StrictNativeStreamReader::new(context, NonZeroUsize::new(32).unwrap(), Duration::from_secs(2)).unwrap();
    let scope = super::super::NativeKvScope::new(NatsToken::new("scoped").unwrap()).unwrap();
    let key = super::super::NativeKvKey::new("exact").unwrap();
    let sequence = NonZeroU64::new(ack.sequence).unwrap();
    let observed = reader.read_kv_sequence(&scope, &key, sequence).await.unwrap().unwrap();
    assert_eq!(observed.subject.as_str(), "$KV.scoped.exact");
    assert_eq!(observed.headers.get_all("Trogon-Registration-Approval").count(), 2);
    let wrong = super::super::NativeKvKey::new("another").unwrap();
    assert!(matches!(
        reader.read_kv_sequence(&scope, &wrong, sequence).await,
        Err(NativeStreamReadError::InvalidRecord)
    ));
    assert!(matches!(
        reader
            .read_sequence(&NatsToken::new("KV_scoped").unwrap(), sequence)
            .await,
        Err(NativeStreamReadError::InvalidRecord)
    ));
    assert!(matches!(
        reader
            .read_last(&NatsToken::new("KV_scoped").unwrap(), "$KV.scoped.exact")
            .await,
        Err(NativeStreamReadError::InvalidBinding)
    ));
}
