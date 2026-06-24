use super::*;
use futures::StreamExt;

fn make_nats_msg(subject: &str, payload: &[u8]) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: None,
        payload: Bytes::from(payload.to_vec()),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    }
}

#[tokio::test]
async fn mock_js_message_records_signals() {
    let msg = MockJsMessage::new(make_nats_msg("test", b"payload"));
    msg.ack().await.unwrap();
    msg.ack_with(AckKind::Progress).await.unwrap();
    msg.ack_with(AckKind::Term).await.unwrap();
    assert_eq!(
        msg.signals(),
        vec![
            AckKindSnapshot::Ack,
            AckKindSnapshot::AckWith(AckKindValue::Progress),
            AckKindSnapshot::AckWith(AckKindValue::Term),
        ]
    );
}

#[tokio::test]
async fn mock_js_message_records_nak_with_delay() {
    let msg = MockJsMessage::new(make_nats_msg("test", b""));
    msg.ack_with(AckKind::Nak(Some(std::time::Duration::from_secs(5))))
        .await
        .unwrap();
    assert_eq!(
        msg.signals(),
        vec![AckKindSnapshot::AckWith(AckKindValue::Nak(Some(
            std::time::Duration::from_secs(5)
        )))]
    );
}

#[tokio::test]
async fn mock_js_message_records_double_ack() {
    let msg = MockJsMessage::new(make_nats_msg("test", b""));
    msg.double_ack().await.unwrap();
    assert_eq!(msg.signals(), vec![AckKindSnapshot::DoubleAck]);
}

#[tokio::test]
async fn mock_js_message_records_double_ack_with() {
    let msg = MockJsMessage::new(make_nats_msg("test", b""));
    msg.double_ack_with(AckKind::Ack).await.unwrap();
    assert_eq!(msg.signals(), vec![AckKindSnapshot::DoubleAckWith(AckKindValue::Ack)]);
}

#[tokio::test]
async fn mock_context_records_stream_creation() {
    let ctx = MockJetStreamContext::new();
    let config = stream::Config {
        name: "TEST_STREAM".to_string(),
        subjects: vec!["test.>".to_string()],
        ..Default::default()
    };
    ctx.get_or_create_stream(config).await.unwrap();
    assert_eq!(ctx.created_streams().len(), 1);
    assert_eq!(ctx.created_streams()[0].name, "TEST_STREAM");
}

#[tokio::test]
async fn mock_context_fails_when_configured() {
    let ctx = MockJetStreamContext::new();
    ctx.fail_next();
    let result = ctx.get_or_create_stream(stream::Config::default()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mock_publisher_records_publishes() {
    let pub_mock = MockJetStreamPublisher::new();
    let ack = pub_mock
        .publish_with_headers("test.subject".to_string(), HeaderMap::new(), Bytes::from("hello"))
        .await
        .unwrap()
        .await
        .unwrap();
    assert_eq!(ack.sequence, 1);
    assert_eq!(pub_mock.published_subjects(), vec!["test.subject"]);
    assert_eq!(pub_mock.published_payloads(), vec![Bytes::from("hello")]);
}

#[tokio::test]
async fn mock_publisher_increments_sequence() {
    let pub_mock = MockJetStreamPublisher::new();
    let ack1 = pub_mock
        .publish_with_headers("a".to_string(), HeaderMap::new(), Bytes::new())
        .await
        .unwrap()
        .await
        .unwrap();
    let ack2 = pub_mock
        .publish_with_headers("b".to_string(), HeaderMap::new(), Bytes::new())
        .await
        .unwrap()
        .await
        .unwrap();
    assert_eq!(ack1.sequence, 1);
    assert_eq!(ack2.sequence, 2);
}

#[tokio::test]
async fn mock_publish_message_advances_default_sequence_after_explicit_ack() {
    let publisher = MockJetStreamPublishMessage::new();
    publisher.enqueue_ack_with_sequence(10);

    let first = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .payload(Bytes::new())
                .outbound_message("a"),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    let second = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .payload(Bytes::new())
                .outbound_message("b"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    assert_eq!(first.sequence, 10);
    assert_eq!(second.sequence, 11);
}

#[tokio::test]
async fn mock_publish_message_scripts_ack_level_errors() {
    let publisher = MockJetStreamPublishMessage::new();
    publisher.enqueue_wrong_last_sequence();
    publisher.enqueue_ack_error(PublishErrorKind::Other);

    let wrong_last_sequence = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();
    let other = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();

    assert_eq!(wrong_last_sequence.kind(), PublishErrorKind::WrongLastSequence);
    assert_eq!(other.kind(), PublishErrorKind::Other);
    assert_eq!(publisher.published_messages().len(), 2);
}

#[tokio::test]
async fn mock_publish_message_scripts_outer_publish_errors_and_duplicates() {
    let publisher = MockJetStreamPublishMessage::new();
    publisher.enqueue_publish_error(PublishErrorKind::TimedOut);
    publisher.enqueue_duplicate();

    let publish_error = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap_err();
    let duplicate = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    assert_eq!(publish_error.kind(), PublishErrorKind::TimedOut);
    assert_eq!(duplicate.sequence, 1);
    assert!(duplicate.duplicate);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn mock_publish_message_rejects_invalid_expected_sequence_header() {
    let publisher = MockJetStreamPublishMessage::new();

    let error = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .header(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, "not-a-number")
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap_err();

    assert_eq!(error.kind(), PublishErrorKind::WrongLastSequence);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn mock_publish_message_enforces_expected_last_subject_sequence() {
    let publisher = MockJetStreamPublishMessage::new();

    publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("first")
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("second")
                .expected_last_subject_sequence(1)
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    let stale = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("stale")
                .expected_last_subject_sequence(1)
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();

    assert_eq!(stale.kind(), PublishErrorKind::WrongLastSequence);
    assert_eq!(publisher.last_subject_sequence("events.alpha"), 2);
}

#[tokio::test]
async fn mock_publish_message_rejects_duplicate_message_id_without_recording() {
    let publisher = MockJetStreamPublishMessage::new();

    let first = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-1")
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    let duplicate = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-1")
                .payload(Bytes::new())
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    assert_eq!(first.sequence, 1);
    assert_eq!(duplicate.sequence, 1);
    assert!(duplicate.duplicate);
    assert_eq!(publisher.published_messages().len(), 1);
}

#[tokio::test]
async fn mock_publish_message_stages_atomic_batch_until_commit() {
    let publisher = MockJetStreamPublishMessage::new();

    let member = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-1")
                .header(NATS_BATCH_ID, "batch-1")
                .payload(Bytes::from_static(b"one"))
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(member.kind(), PublishErrorKind::Other);
    assert!(publisher.published_messages().is_empty());

    let commit = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-2")
                .header(NATS_BATCH_ID, "batch-1")
                .header(NATS_BATCH_COMMIT, "1")
                .payload(Bytes::from_static(b"two"))
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    assert_eq!(commit.sequence, 2);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].payload, Bytes::from_static(b"one"));
    assert_eq!(messages[1].payload, Bytes::from_static(b"two"));
}

#[tokio::test]
async fn mock_publish_message_drops_pending_batch_on_conflict() {
    let publisher = MockJetStreamPublishMessage::new();

    publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-1")
                .header(NATS_BATCH_ID, "batch-1")
                .payload(Bytes::from_static(b"one"))
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();
    let conflict = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-2")
                .expected_last_subject_sequence(99)
                .header(NATS_BATCH_ID, "batch-1")
                .payload(Bytes::from_static(b"two"))
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();
    let commit = publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-3")
                .header(NATS_BATCH_ID, "batch-1")
                .header(NATS_BATCH_COMMIT, "1")
                .payload(Bytes::from_static(b"three"))
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    assert_eq!(conflict.kind(), PublishErrorKind::WrongLastSequence);
    assert_eq!(commit.sequence, 2);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].payload, Bytes::from_static(b"three"));
}

#[tokio::test]
async fn mock_publish_message_reads_committed_messages() {
    let publisher = MockJetStreamPublishMessage::new();
    publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-1")
                .payload(Bytes::from_static(b"one"))
                .outbound_message("events.alpha"),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    publisher
        .publish_message(
            async_nats::jetstream::message::PublishMessage::build()
                .message_id("event-2")
                .payload(Bytes::from_static(b"two"))
                .outbound_message("events.beta"),
        )
        .await
        .unwrap()
        .await
        .unwrap();

    let info = publisher.get_info().await.unwrap();
    let raw = publisher.get_raw_message(1).await.unwrap();
    let latest_alpha = publisher.get_last_raw_message_by_subject("events.alpha").await.unwrap();

    assert_eq!(info.state.last_sequence, 2);
    assert_eq!(raw.payload, Bytes::from_static(b"one"));
    assert_eq!(latest_alpha.sequence, 1);
}

#[tokio::test]
async fn mock_publish_message_missing_reads_return_not_found() {
    let publisher = MockJetStreamPublishMessage::new();

    let latest = publisher
        .get_last_raw_message_by_subject("events.missing")
        .await
        .unwrap_err();
    let raw = publisher.get_raw_message(99).await.unwrap_err();

    assert_eq!(latest.kind(), LastRawMessageErrorKind::NoMessageFound);
    assert_eq!(raw.kind(), LastRawMessageErrorKind::NoMessageFound);
}

#[tokio::test]
async fn mock_publisher_fails_when_configured() {
    let pub_mock = MockJetStreamPublisher::new();
    pub_mock.fail_next_js_publish();
    let result = pub_mock
        .publish_with_headers("test".to_string(), HeaderMap::new(), Bytes::new())
        .await;
    assert!(result.is_err());

    let result = pub_mock
        .publish_with_headers("test".to_string(), HeaderMap::new(), Bytes::new())
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn mock_consumer_factory_returns_consumers_in_order() {
    let factory = MockJetStreamConsumerFactory::new();
    let (consumer1, _tx1) = MockJetStreamConsumer::new();
    let (consumer2, _tx2) = MockJetStreamConsumer::new();
    factory.add_consumer(consumer1);
    factory.add_consumer(consumer2);

    let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();
    let _c1 = stream.create_consumer(pull::Config::default()).await.unwrap();
    let _c2 = stream.create_consumer(pull::Config::default()).await.unwrap();

    let result = stream.create_consumer(pull::Config::default()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn mock_consumer_factory_records_get_stream_calls() {
    let factory = MockJetStreamConsumerFactory::new();

    JetStreamGetStream::get_stream(&factory, "STREAM_A").await.unwrap();
    JetStreamGetStream::get_stream(&factory, "STREAM_B").await.unwrap();

    assert_eq!(factory.get_stream_calls(), vec!["STREAM_A", "STREAM_B"]);
}

#[tokio::test]
async fn mock_stream_returns_last_raw_messages_by_subject() {
    let factory = MockJetStreamConsumerFactory::new();
    factory.add_last_raw_message(StreamMessage {
        subject: "stored.subject".into(),
        sequence: 42,
        headers: HeaderMap::new(),
        payload: Bytes::from_static(br#"{"ok":true}"#),
        time: OffsetDateTime::UNIX_EPOCH,
    });
    let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

    let message = stream
        .get_last_raw_message_by_subject("notion.subscription.verification")
        .await
        .unwrap();

    assert_eq!(message.subject.as_str(), "stored.subject");
    assert_eq!(message.sequence, 42);
    assert_eq!(message.payload, Bytes::from_static(br#"{"ok":true}"#));
    assert_eq!(
        factory.last_raw_message_subjects(),
        vec!["notion.subscription.verification"]
    );
}

#[tokio::test]
async fn mock_stream_defaults_to_no_message_found() {
    let factory = MockJetStreamConsumerFactory::new();
    let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

    let error = stream.get_last_raw_message_by_subject("subject").await.unwrap_err();

    assert_eq!(error.kind(), LastRawMessageErrorKind::NoMessageFound);
    assert_eq!(error.to_string(), "no message found");
}

#[tokio::test]
async fn mock_stream_returns_configured_last_raw_message_error() {
    let factory = MockJetStreamConsumerFactory::new();
    factory.add_last_raw_message_error(LastRawMessageError::new(LastRawMessageErrorKind::Other));
    let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

    let error = stream.get_last_raw_message_by_subject("subject").await.unwrap_err();

    assert_eq!(error.kind(), LastRawMessageErrorKind::Other);
    assert_eq!(error.to_string(), "failed to get last raw message");
}

#[tokio::test]
async fn mock_stream_info_and_raw_messages_are_configurable_from_factory() {
    let factory = MockJetStreamConsumerFactory::new();
    factory.set_info(mock_stream_info(3, 9));
    factory.add_raw_message(
        7,
        StreamMessage {
            subject: "stored.subject".into(),
            sequence: 7,
            headers: HeaderMap::new(),
            payload: Bytes::from_static(b"seven"),
            time: OffsetDateTime::UNIX_EPOCH,
        },
    );
    factory.add_raw_message_error(8, LastRawMessageErrorKind::Other);
    let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

    let info = stream.get_info().await.unwrap();
    let message = stream.get_raw_message(7).await.unwrap();
    let configured_error = stream.get_raw_message(8).await.unwrap_err();
    let missing = stream.get_raw_message(9).await.unwrap_err();

    assert_eq!(info.state.messages, 3);
    assert_eq!(info.state.last_sequence, 9);
    assert_eq!(message.payload, Bytes::from_static(b"seven"));
    assert_eq!(configured_error.kind(), LastRawMessageErrorKind::Other);
    assert_eq!(missing.kind(), LastRawMessageErrorKind::NoMessageFound);
    assert_eq!(factory.raw_message_calls(), vec![7, 8, 9]);
}

#[tokio::test]
async fn mock_stream_info_and_raw_messages_are_configurable_from_stream() {
    let factory = MockJetStreamConsumerFactory::new();
    let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();
    let default_info_error = stream.get_info().await.unwrap_err();
    stream.set_info(mock_stream_info(1, 4));
    stream.add_raw_message(
        4,
        StreamMessage {
            subject: "stored.subject".into(),
            sequence: 4,
            headers: HeaderMap::new(),
            payload: Bytes::from_static(b"four"),
            time: OffsetDateTime::UNIX_EPOCH,
        },
    );
    stream.add_raw_message_error(5, LastRawMessageErrorKind::Other);

    let info = stream.get_info().await.unwrap();
    let message = stream.get_raw_message(4).await.unwrap();
    let error = stream.get_raw_message(5).await.unwrap_err();

    assert_eq!(default_info_error.kind(), RequestErrorKind::Other);
    assert_eq!(info.state.first_sequence, 1);
    assert_eq!(message.sequence, 4);
    assert_eq!(error.kind(), LastRawMessageErrorKind::Other);
    assert_eq!(stream.raw_message_calls(), vec![4, 5]);
}

#[tokio::test]
async fn mock_kv_status_uses_configured_bucket_name() {
    let store = MockJetStreamKvStore::new();
    store.set_bucket_name("custom-bucket");

    let status = store.status().await.unwrap();

    assert_eq!(status.bucket, "custom-bucket");
    assert_eq!(status.info.config.name, "KV_custom-bucket");
}

#[tokio::test]
async fn mock_kv_store_records_core_operations_and_queued_results() {
    let store = MockJetStreamKvStore::new();
    store.enqueue_update_result(Ok(11));
    store.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    store.enqueue_delete_result(Ok(()));
    store.enqueue_delete_result(Err(kv::DeleteErrorKind::TimedOut));
    store.enqueue_create_result(Ok(12));
    store.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));

    assert_eq!(
        store
            .create_with_ttl("ttl-key", Bytes::from_static(b"ttl"), Duration::from_secs(5))
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.update("key", Bytes::from_static(b"value"), 10).await.unwrap(), 11);
    assert_eq!(
        store
            .update("key", Bytes::from_static(b"value"), 11)
            .await
            .unwrap_err()
            .kind(),
        kv::UpdateErrorKind::WrongLastRevision
    );
    store.delete_expect_revision("key", Some(11)).await.unwrap();
    assert_eq!(
        store.delete_expect_revision("key", None).await.unwrap_err().kind(),
        kv::DeleteErrorKind::TimedOut
    );
    assert_eq!(store.create("new", Bytes::from_static(b"value")).await.unwrap(), 12);
    assert_eq!(
        store
            .create("existing", Bytes::from_static(b"value"))
            .await
            .unwrap_err()
            .kind(),
        kv::CreateErrorKind::AlreadyExists
    );
    assert_eq!(store.create("fallback", Bytes::new()).await.unwrap(), 1);

    assert_eq!(
        store.create_with_ttl_calls(),
        vec![(
            "ttl-key".to_string(),
            Bytes::from_static(b"ttl"),
            Duration::from_secs(5)
        )]
    );
    assert_eq!(
        store.update_calls(),
        vec![
            ("key".to_string(), Bytes::from_static(b"value"), 10),
            ("key".to_string(), Bytes::from_static(b"value"), 11),
        ]
    );
    assert_eq!(
        store.delete_calls(),
        vec![("key".to_string(), Some(11)), ("key".to_string(), None)]
    );
    assert_eq!(
        store.create_calls(),
        vec![
            ("new".to_string(), Bytes::from_static(b"value")),
            ("existing".to_string(), Bytes::from_static(b"value")),
            ("fallback".to_string(), Bytes::new()),
        ]
    );
}

#[tokio::test]
async fn mock_kv_store_reads_entries_gets_and_keys() {
    let store = MockJetStreamKvStore::new();
    store.set_bucket_name("custom-bucket");
    store.enqueue_entry(Bytes::from_static(b"entry"), 21, kv::Operation::Put);
    store.enqueue_entry_none();
    store.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    store.enqueue_get_some(Bytes::from_static(b"get"));
    store.enqueue_get_none();
    store.enqueue_get_error(kv::EntryErrorKind::Other);
    store.set_keys_result(Ok(vec!["a".to_string(), "b".to_string()]));

    let entry = store.entry("entry-key".to_string()).await.unwrap().unwrap();
    let missing_entry = store.entry("missing-entry".to_string()).await.unwrap();
    let entry_error = store.entry("bad-entry".to_string()).await.unwrap_err();
    let value = store.get("get-key".to_string()).await.unwrap().unwrap();
    let missing_value = store.get("missing-get".to_string()).await.unwrap();
    let get_error = store.get("bad-get".to_string()).await.unwrap_err();
    let keys: Vec<_> = store.keys().await.unwrap().collect().await;

    assert_eq!(entry.bucket, "custom-bucket");
    assert_eq!(entry.key, "entry-key");
    assert_eq!(entry.value, Bytes::from_static(b"entry"));
    assert_eq!(entry.revision, 21);
    assert_eq!(entry.operation, kv::Operation::Put);
    assert!(missing_entry.is_none());
    assert_eq!(entry_error.kind(), kv::EntryErrorKind::TimedOut);
    assert_eq!(value, Bytes::from_static(b"get"));
    assert!(missing_value.is_none());
    assert_eq!(get_error.kind(), kv::EntryErrorKind::Other);
    assert_eq!(keys.into_iter().map(Result::unwrap).collect::<Vec<_>>(), vec!["a", "b"]);
    assert_eq!(store.entry_calls(), vec!["entry-key", "missing-entry", "bad-entry"]);
    assert_eq!(store.get_calls(), vec!["get-key", "missing-get", "bad-get"]);
    assert_eq!(store.keys_calls(), 1);
}

#[tokio::test]
async fn mock_kv_store_surfaces_configured_errors() {
    let store = MockJetStreamKvStore::new();
    store.fail_status(kv::StatusErrorKind::TimedOut);
    store.set_create_with_ttl_result(Err(kv::CreateErrorKind::InvalidKey));
    store.set_update_result(Err(kv::UpdateErrorKind::InvalidKey));
    store.set_delete_result(Err(kv::DeleteErrorKind::InvalidKey));
    store.set_keys_result(Err(kv::WatchErrorKind::ConsumerCreate));

    assert_eq!(store.status().await.unwrap_err().kind(), kv::StatusErrorKind::TimedOut);
    assert_eq!(
        store
            .create_with_ttl("", Bytes::new(), Duration::from_secs(1))
            .await
            .unwrap_err()
            .kind(),
        kv::CreateErrorKind::InvalidKey
    );
    assert_eq!(
        store.update("", Bytes::new(), 1).await.unwrap_err().kind(),
        kv::UpdateErrorKind::InvalidKey
    );
    assert_eq!(
        store.delete_expect_revision("", None).await.unwrap_err().kind(),
        kv::DeleteErrorKind::InvalidKey
    );
    assert_eq!(
        store.keys().await.unwrap_err().kind(),
        kv::WatchErrorKind::ConsumerCreate
    );
}

#[tokio::test]
async fn mock_consumer_messages_streams() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let msg = MockJsMessage::new(make_nats_msg("test.subject", b"data"));
    tx.unbounded_send(Ok(msg)).unwrap();
    drop(tx);

    let mut stream = JetStreamConsumer::messages(&consumer).await.unwrap();
    let received = stream.next().await.unwrap().unwrap();
    assert_eq!(received.message().subject.as_str(), "test.subject");
    assert_eq!(received.message().payload.as_ref(), b"data");
}

#[test]
fn mock_context_default() {
    let ctx = MockJetStreamContext::default();
    assert!(ctx.created_streams().is_empty());
}

#[test]
fn mock_publisher_default() {
    let pub_mock = MockJetStreamPublisher::default();
    assert!(pub_mock.published_subjects().is_empty());
}

#[test]
fn mock_consumer_factory_default() {
    let _factory = MockJetStreamConsumerFactory::default();
}

#[tokio::test]
async fn mock_js_message_records_nak() {
    let msg = MockJsMessage::new(make_nats_msg("test", b""));
    msg.ack_with(AckKind::Nak(None)).await.unwrap();
    assert_eq!(msg.signals(), vec![AckKindSnapshot::AckWith(AckKindValue::Nak(None))]);
}

#[tokio::test]
async fn mock_publisher_fail_js_publish_count() {
    let pub_mock = MockJetStreamPublisher::new();
    pub_mock.fail_js_publish_count(2);
    assert!(
        pub_mock
            .publish_with_headers("a".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .is_err()
    );
    assert!(
        pub_mock
            .publish_with_headers("b".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .is_err()
    );
    assert!(
        pub_mock
            .publish_with_headers("c".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .unwrap()
            .await
            .is_ok()
    );
}

#[test]
fn mock_js_message_payload_subject_headers_reply() {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("X-Test", "value");
    let msg = async_nats::Message {
        subject: "test.subject".into(),
        reply: Some("_INBOX.reply".into()),
        payload: Bytes::from("hello"),
        headers: Some(headers),
        status: None,
        description: None,
        length: 5,
    };
    let mock = MockJsMessage::new(msg);

    let inner = mock.message();
    assert_eq!(inner.payload.as_ref(), b"hello");
    assert_eq!(inner.subject.as_str(), "test.subject");
    assert!(inner.headers.is_some());
    assert_eq!(inner.reply.as_ref().map(|s| s.as_str()), Some("_INBOX.reply"));
}

#[test]
fn mock_js_message_no_headers_no_reply() {
    let msg = make_nats_msg("sub", b"data");
    let mock = MockJsMessage::new(msg);

    let inner = mock.message();
    assert!(inner.headers.is_none());
    assert!(inner.reply.is_none());
}

#[test]
fn mock_consumer_factory_clone() {
    let factory = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    factory.add_consumer(consumer);
    let cloned = factory.clone();
    assert!(cloned.consumers.lock().unwrap().len() == 1);
}

#[tokio::test]
async fn mock_js_message_failing_signals() {
    let msg = MockJsMessage::with_failing_signals(make_nats_msg("test", b""));
    assert!(msg.ack().await.is_err());
    assert!(msg.ack_with(AckKind::Term).await.is_err());
    assert!(msg.double_ack().await.is_err());
    assert!(msg.double_ack_with(AckKind::Ack).await.is_err());
}

#[tokio::test]
async fn mock_consumer_messages_returns_stream() {
    let (consumer, _tx) = MockJetStreamConsumer::new();
    let result = JetStreamConsumer::messages(&consumer).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn mock_consumer_messages_called_twice_returns_error() {
    let (consumer, _tx) = MockJetStreamConsumer::new();
    let _first = JetStreamConsumer::messages(&consumer).await.unwrap();
    let second = JetStreamConsumer::messages(&consumer).await;
    assert!(second.is_err());
}

#[tokio::test]
async fn mock_js_message_ack_with_next() {
    let msg = MockJsMessage::new(make_nats_msg("test", b""));
    msg.ack_with(AckKind::Next).await.unwrap();
    assert_eq!(msg.signals(), vec![AckKindSnapshot::AckWith(AckKindValue::Next)]);
}

#[test]
fn mock_object_store_default() {
    let store = MockObjectStore::default();
    assert!(store.stored_objects().is_empty());
}

#[tokio::test]
async fn mock_object_store_fail_next_get() {
    let store = MockObjectStore::new();
    store.seed("key", Bytes::from("data"));
    store.fail_next_get();
    let result = ObjectStoreGet::get(&store, "key").await;
    assert!(result.is_err());

    let result = ObjectStoreGet::get(&store, "key").await;
    assert!(result.is_ok());
}

#[test]
fn mock_jetstream_purger_default_and_failures() {
    let purger = MockJetStreamPurger::default();
    assert!(purger.purged_subjects().is_empty());
}

#[tokio::test]
async fn mock_jetstream_purger_records_success_and_failure() {
    let purger = MockJetStreamPurger::new();
    purger.purge_subject_messages("test.subject").await.unwrap();
    assert_eq!(purger.purged_subjects(), vec!["test.subject".to_string()]);

    purger.fail_purge_count(1);
    assert!(purger.purge_subject_messages("retry.subject").await.is_err());
    purger.purge_subject_messages("retry.subject").await.unwrap();
}

#[test]
fn mock_jetstream_kv_store_default() {
    let _ = MockJetStreamKvStore::default();
}

#[test]
fn mock_jetstream_kv_client_default() {
    let _ = MockJetStreamKvClient::default();
}
