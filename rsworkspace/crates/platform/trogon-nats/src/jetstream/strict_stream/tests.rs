use super::*;
use crate::test_support::{CoreTestServer, JetStreamTestServer};
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
    assert_eq!(
        reader
            .read_next(&stream, "events.one", NonZeroU64::new(first.sequence + 1).unwrap())
            .await
            .unwrap()
            .unwrap()
            .sequence,
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

#[tokio::test]
async fn native_stream_reader_keeps_unavailable_protocol_reply_distinct_from_corrupt_record() {
    let server = JetStreamTestServer::start().await;
    let context = async_nats::jetstream::with_prefix(server.client().await, "unavailable");
    let reader =
        StrictNativeStreamReader::new(context, NonZeroUsize::new(32).unwrap(), Duration::from_secs(2)).unwrap();
    let error = reader
        .read_sequence(&NatsToken::new("RAW_EVENTS").unwrap(), NonZeroU64::new(1).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        NativeStreamReadError::ProtocolStatus(async_nats::StatusCode::NO_RESPONDERS)
    ));
}

#[tokio::test]
async fn native_stream_reader_reads_the_next_matching_record_and_last_scoped_kv_record() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_stream(stream::Config {
            name: "KV_history".into(),
            subjects: vec!["$KV.history.>".into()],
            storage: stream::StorageType::File,
            ..Default::default()
        })
        .await
        .unwrap();
    let first = context
        .publish("$KV.history.entry", vec![1].into())
        .await
        .unwrap()
        .await
        .unwrap();
    context
        .publish("$KV.history.other", vec![2].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let last = context
        .publish("$KV.history.entry", vec![3].into())
        .await
        .unwrap()
        .await
        .unwrap();
    let reader =
        StrictNativeStreamReader::new(context, NonZeroUsize::new(32).unwrap(), Duration::from_secs(2)).unwrap();
    let scope = super::super::NativeKvScope::new(NatsToken::new("history").unwrap()).unwrap();
    let key = super::super::NativeKvKey::new("entry").unwrap();
    assert_eq!(
        reader.read_kv_last(&scope, &key).await.unwrap().unwrap().sequence,
        last.sequence
    );
    assert!(
        reader
            .read_kv_last(&scope, &super::super::NativeKvKey::new("missing").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let stream = NatsToken::new("KV_history").unwrap();
    assert!(
        reader
            .read_next(&stream, "events.missing", NonZeroU64::new(first.sequence).unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

async fn reply_once(context: &Context, payload: &'static [u8]) -> NativeStreamReadError {
    let client = context.client();
    let mut requests = client.subscribe("native.test.STREAM.MSG.GET.>").await.unwrap();
    client.flush().await.unwrap();
    let task = tokio::spawn(async move {
        let message = requests.next().await.unwrap();
        client
            .publish(message.reply.unwrap(), bytes::Bytes::from_static(payload))
            .await
            .unwrap();
        client.flush().await.unwrap();
    });
    let reader = StrictNativeStreamReader::new(
        async_nats::jetstream::with_prefix(context.client(), "native.test"),
        NonZeroUsize::new(1).unwrap(),
        Duration::from_secs(2),
    )
    .unwrap();
    let error = reader
        .read_sequence(&NatsToken::new("RAW_EVENTS").unwrap(), NonZeroU64::new(1).unwrap())
        .await
        .unwrap_err();
    task.await.unwrap();
    error
}

#[tokio::test]
async fn native_stream_reader_refuses_malformed_api_and_record_data() {
    let server = CoreTestServer::start().await;
    let context = async_nats::jetstream::new(async_nats::connect(server.address().to_owned()).await.unwrap());
    for payload in [
        br#"{"message":{"subject":"events.one","seq":0,"data":"AQ==","time":"2026-01-01T00:00:00Z"}}"#.as_slice(),
        br#"{"message":{"subject":"events.one","seq":2,"data":"AQ==","time":"2026-01-01T00:00:00Z"}}"#,
        br#"{"message":{"subject":"events.*","seq":1,"data":"AQ==","time":"2026-01-01T00:00:00Z"}}"#,
        br#"{"message":{"subject":"events.one","seq":1,"data":"??","time":"2026-01-01T00:00:00Z"}}"#,
    ] {
        assert!(matches!(
            reply_once(&context, payload).await,
            NativeStreamReadError::InvalidRecord
        ));
    }
    assert!(matches!(
        reply_once(&context, b"not-json").await,
        NativeStreamReadError::Json(_)
    ));
    assert!(matches!(
        reply_once(
            &context,
            br#"{"error":{"code":500,"err_code":10052,"description":"invalid profile"}}"#
        )
        .await,
        NativeStreamReadError::Api(_)
    ));
    assert!(matches!(
        reply_once(
            &context,
            br#"{"message":{"subject":"events.one","seq":1,"data":"AQI=","time":"2026-01-01T00:00:00Z"}}"#
        )
        .await,
        NativeStreamReadError::Limit
    ));
}

#[test]
fn native_stored_header_parser_enforces_original_wire_bounds_and_grammar() {
    assert!(parse_native_stored_headers(b"NATS/1.0\r\n\r\n").unwrap().is_empty());
    assert!(matches!(
        parse_native_stored_headers(&vec![b'x'; 16 * 1024 + 1]),
        Err(NativeStreamReadError::Limit)
    ));
    for raw in [
        b"NATS/1.0\r\nName: \xff\r\n\r\n".as_slice(),
        b"NATS/1.0\r\nNoColon\r\n\r\n",
        b"NATS/1.0\r\n: empty-name\r\n\r\n",
        b"NATS/1.0\r\nbad name: value\r\n\r\n",
        b"NATS/1.0\r\nName: bad\x00value\r\n\r\n",
        b"NATS/1.0\r\n Name: folded\r\n\r\n",
        b"NATS/1.0\r\n\r\n\r\n",
        b"NATS/1.0\r\nName: value\r\n",
    ] {
        assert!(matches!(
            parse_native_stored_headers(raw),
            Err(NativeStreamReadError::InvalidRecord)
        ));
    }
    let too_many = format!("NATS/1.0\r\n{}\r\n", "Name: x\r\n".repeat(129));
    assert!(matches!(
        parse_native_stored_headers(too_many.as_bytes()),
        Err(NativeStreamReadError::InvalidRecord)
    ));
    let too_wide = format!("NATS/1.0\r\nName: {}\r\n\r\n", "x".repeat(2049));
    assert!(matches!(
        parse_native_stored_headers(too_wide.as_bytes()),
        Err(NativeStreamReadError::InvalidRecord)
    ));
    let headers = parse_native_stored_headers(b"NATS/1.0\r\nName:value\r\nOther: \ttab\r\n\r\n").unwrap();
    assert_eq!(headers.get("Name").unwrap().as_str(), "value");
    assert_eq!(headers.get("Other").unwrap().as_str(), "\ttab");
}

#[tokio::test]
async fn native_stream_reader_validates_its_profile_before_requesting_data() {
    let server = CoreTestServer::start().await;
    let context = async_nats::jetstream::new(async_nats::connect(server.address().to_owned()).await.unwrap());
    for (maximum, timeout) in [
        (64 * 1024 * 1024 + 1, Duration::from_secs(1)),
        (1, Duration::ZERO),
        (1, Duration::from_secs(31)),
    ] {
        assert!(matches!(
            StrictNativeStreamReader::new(context.clone(), NonZeroUsize::new(maximum).unwrap(), timeout),
            Err(NativeStreamReadError::InvalidBinding)
        ));
    }
    let reader =
        StrictNativeStreamReader::new(context, NonZeroUsize::new(32).unwrap(), Duration::from_secs(1)).unwrap();
    assert!(matches!(
        reader
            .read_sequence(&NatsToken::new("stream/name").unwrap(), NonZeroU64::new(1).unwrap())
            .await,
        Err(NativeStreamReadError::InvalidBinding)
    ));
}

#[tokio::test]
async fn native_stream_reader_refuses_a_next_record_before_the_requested_lower_bound() {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address().to_owned()).await.unwrap();
    let mut requests = client.subscribe("native.test.STREAM.MSG.GET.>").await.unwrap();
    client.flush().await.unwrap();
    let responder = client.clone();
    let task = tokio::spawn(async move {
        let request = requests.next().await.unwrap();
        responder
            .publish(
                request.reply.unwrap(),
                bytes::Bytes::from_static(
                    br#"{"message":{"subject":"events.one","seq":1,"data":"AQ==","time":"2026-01-01T00:00:00Z"}}"#,
                ),
            )
            .await
            .unwrap();
        responder.flush().await.unwrap();
    });
    let reader = StrictNativeStreamReader::new(
        async_nats::jetstream::with_prefix(client, "native.test"),
        NonZeroUsize::new(32).unwrap(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(matches!(
        reader
            .read_next(
                &NatsToken::new("RAW_EVENTS").unwrap(),
                "events.one",
                NonZeroU64::new(2).unwrap()
            )
            .await,
        Err(NativeStreamReadError::InvalidRecord)
    ));
    task.await.unwrap();
}

#[tokio::test]
async fn native_stream_reader_bounds_the_wire_response_before_deserialization() {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address().to_owned()).await.unwrap();
    let mut requests = client.subscribe("native.test.STREAM.MSG.GET.>").await.unwrap();
    client.flush().await.unwrap();
    let responder = client.clone();
    let task = tokio::spawn(async move {
        let request = requests.next().await.unwrap();
        responder
            .publish(request.reply.unwrap(), vec![b'x'; 40_000].into())
            .await
            .unwrap();
        responder.flush().await.unwrap();
    });
    let reader = StrictNativeStreamReader::new(
        async_nats::jetstream::with_prefix(client, "native.test"),
        NonZeroUsize::new(32).unwrap(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(matches!(
        reader
            .read_sequence(&NatsToken::new("RAW_EVENTS").unwrap(), NonZeroU64::new(1).unwrap())
            .await,
        Err(NativeStreamReadError::Limit)
    ));
    task.await.unwrap();
}
