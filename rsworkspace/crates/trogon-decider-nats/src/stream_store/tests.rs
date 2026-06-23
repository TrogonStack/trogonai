use async_nats::{
        HeaderMap,
        header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_ID},
        jetstream::{
            context::PublishErrorKind,
            message::StreamMessage,
            stream::{LastRawMessageError, LastRawMessageErrorKind},
        },
    };
    use bytes::Bytes;
    use time::OffsetDateTime;
    use trogon_nats::jetstream::JetStreamGetStream;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumerFactory, MockJetStreamPublishMessage};
    use uuid::Uuid;

    use trogon_decider_runtime::{Event, EventId, Headers, StreamPosition};

    use super::{
        NATS_BATCH_COMMIT, NATS_BATCH_ID, NATS_BATCH_SEQUENCE, StreamStoreError, StreamSubject,
        TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, append_stream, build_publish_message, headers_from_nats_headers,
        read_stream, read_stream_range, subject_current_position,
    };

    #[test]
    fn stream_subject_accepts_valid_subjects() {
        let subject = StreamSubject::new("scheduler.schedules.events.backup").unwrap();

        assert_eq!(subject.as_str(), "scheduler.schedules.events.backup");
        assert_eq!(subject.to_string(), "scheduler.schedules.events.backup");
    }

    #[test]
    fn stream_subject_rejects_malformed_subjects() {
        for subject in ["", ".events", "events.", "events..backup", "events backup"] {
            assert!(StreamSubject::new(subject).is_err(), "{subject}");
        }
    }

    #[test]
    fn build_publish_message_sets_trogon_event_type_header() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogonai.scheduler.schedules.v1.ScheduleCreated".to_string(),
            content: Vec::new(),
            headers: Headers::empty(),
        };

        let message = build_publish_message(&event, Vec::new(), Some(0), "batch-1", 0, 1)
            .outbound_message("scheduler.schedules.events.backup");
        let headers = message.headers.unwrap_or_default();

        assert_eq!(
            headers.get(TROGON_EVENT_TYPE).map(|value| value.as_str()),
            Some("trogonai.scheduler.schedules.v1.ScheduleCreated")
        );
        assert_eq!(
            headers.get(NATS_MESSAGE_ID).map(|value| value.as_str()),
            Some("00000000-0000-0000-0000-000000000001")
        );
    }

    #[test]
    fn build_publish_message_maps_headers_to_trogon_headers() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogonai.scheduler.schedules.v1.ScheduleCreated".to_string(),
            content: Vec::new(),
            headers: Headers::from_entries([("trace-id", "trace-1"), ("tenant", "trogon")]).unwrap(),
        };

        let headers = build_publish_message(&event, Vec::new(), None, "batch-1", 0, 1)
            .outbound_message("scheduler.schedules.events.backup")
            .headers
            .unwrap_or_default();

        assert_eq!(
            headers
                .get(format!("{TROGON_EVENT_HEADER_PREFIX}trace-id").as_str())
                .map(|value| value.as_str()),
            Some("trace-1")
        );
        assert_eq!(
            headers
                .get(format!("{TROGON_EVENT_HEADER_PREFIX}tenant").as_str())
                .map(|value| value.as_str()),
            Some("trogon")
        );
    }

    #[test]
    fn headers_from_nats_headers_reads_trogon_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(format!("{TROGON_EVENT_HEADER_PREFIX}trace-id"), "trace-1");
        headers.insert("Trogon-Event-Type", "test.event");
        headers.insert("Nats-Msg-Id", "00000000-0000-0000-0000-000000000001");

        let parsed_headers = headers_from_nats_headers(&headers).unwrap();

        assert_eq!(
            parsed_headers.get("trace-id").map(|value| value.as_str()),
            Some("trace-1")
        );
        assert_eq!(parsed_headers.len(), 1);
    }

    #[test]
    fn build_publish_message_sets_atomic_batch_occ_on_first_message_only() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogonai.scheduler.schedules.v1.ScheduleCreated".to_string(),
            content: Vec::new(),
            headers: Headers::empty(),
        };

        let first = build_publish_message(&event, Vec::new(), Some(8), "batch-1", 0, 2)
            .outbound_message("scheduler.schedules.events.backup")
            .headers
            .unwrap_or_default();
        let second = build_publish_message(&event, Vec::new(), Some(8), "batch-1", 1, 2)
            .outbound_message("scheduler.schedules.events.backup")
            .headers
            .unwrap_or_default();

        assert_eq!(
            first
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            Some("8")
        );
        assert_eq!(
            second
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            None
        );
        assert_eq!(first.get(NATS_BATCH_COMMIT).map(|value| value.as_str()), None);
        assert_eq!(second.get(NATS_BATCH_COMMIT).map(|value| value.as_str()), Some("1"));
    }

    #[test]
    fn build_publish_message_omits_occ_header_without_expected_sequence() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogonai.scheduler.schedules.v1.ScheduleCreated".to_string(),
            content: Vec::new(),
            headers: Headers::empty(),
        };

        let headers = build_publish_message(&event, Vec::new(), None, "batch-1", 0, 1)
            .outbound_message("scheduler.schedules.events.backup")
            .headers
            .unwrap_or_default();

        assert_eq!(
            headers
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            None
        );
    }

    #[test]
    fn read_stream_range_rejects_empty_ranges() {
        assert!(read_stream_range_bounds(0, 1));
        assert!(read_stream_range_bounds(1, 0));
        assert!(read_stream_range_bounds(2, 1));
        assert!(!read_stream_range_bounds(1, 1));
    }

    fn read_stream_range_bounds(from_sequence: u64, to_sequence: u64) -> bool {
        from_sequence == 0 || to_sequence == 0 || from_sequence > to_sequence
    }

    fn make_event(id: u128, content: &[u8]) -> Event {
        Event {
            id: EventId::from(Uuid::from_u128(id)),
            r#type: "test.event.v1".to_string(),
            content: content.to_vec(),
            headers: Headers::empty(),
        }
    }

    fn make_subject(value: &str) -> StreamSubject {
        StreamSubject::new(value).expect("subject must be valid")
    }

    fn make_stream_message(subject: &str, sequence: u64, event_id: Uuid) -> StreamMessage {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, event_id.to_string().as_str());
        headers.insert(TROGON_EVENT_TYPE, "test.event.v1");
        StreamMessage {
            subject: subject.into(),
            sequence,
            headers,
            payload: Bytes::from_static(b"{}"),
            time: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn make_stream_info(last_sequence: u64) -> async_nats::jetstream::stream::Info {
        serde_json::from_value(serde_json::json!({
            "config": {
                "name": "TEST_STREAM",
                "subjects": [],
                "retention": "limits",
                "max_consumers": -1,
                "max_msgs": -1,
                "max_bytes": -1,
                "discard": "old",
                "max_age": 0,
                "storage": "file",
                "num_replicas": 1
            },
            "created": "1970-01-01T00:00:00Z",
            "state": {
                "messages": 0_u64,
                "bytes": 0_u64,
                "first_seq": 0_u64,
                "first_ts": "1970-01-01T00:00:00Z",
                "last_seq": last_sequence,
                "last_ts": "1970-01-01T00:00:00Z",
                "consumer_count": 0_usize,
                "num_subjects": 0_u64
            },
            "cluster": null,
            "mirror": null,
            "sources": []
        }))
        .expect("test stream info must be valid")
    }

    #[tokio::test]
    async fn append_stream_publishes_single_event_and_returns_position() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_ack_with_sequence(42);

        let position = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"payload")])
            .await
            .expect("publish should succeed");

        assert_eq!(position, StreamPosition::try_new(42).unwrap());
        let messages = js.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "test.events");
        let headers = messages[0].headers.clone().unwrap_or_default();
        assert_eq!(headers.get(NATS_BATCH_COMMIT).map(|v| v.as_str()), Some("1"));
        assert_eq!(headers.get(NATS_BATCH_SEQUENCE).map(|v| v.as_str()), Some("1"));
    }

    #[tokio::test]
    async fn append_stream_publishes_batch_with_shared_batch_id() {
        let js = MockJetStreamPublishMessage::new();

        let position = append_stream(
            &js,
            make_subject("test.events"),
            Some(0),
            &[make_event(1, b"first"), make_event(2, b"second")],
        )
        .await
        .expect("publish should succeed");

        assert_eq!(position, StreamPosition::try_new(2).unwrap());
        let messages = js.published_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sequence, 1);
        assert_eq!(messages[1].sequence, 2);
        let first_headers = messages[0].headers.clone().unwrap_or_default();
        let second_headers = messages[1].headers.clone().unwrap_or_default();
        let first_batch_id = first_headers.get(NATS_BATCH_ID).map(|v| v.as_str().to_string());
        let second_batch_id = second_headers.get(NATS_BATCH_ID).map(|v| v.as_str().to_string());
        assert!(first_batch_id.is_some());
        assert_eq!(first_batch_id, second_batch_id);
        assert_eq!(
            first_headers
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|v| v.as_str()),
            Some("0")
        );
        assert_eq!(first_headers.get(NATS_BATCH_COMMIT).map(|v| v.as_str()), None);
        assert_eq!(second_headers.get(NATS_BATCH_COMMIT).map(|v| v.as_str()), Some("1"));
    }

    #[tokio::test]
    async fn append_stream_uses_semantic_subject_sequence_guard() {
        let js = MockJetStreamPublishMessage::new();

        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(0),
            &[make_event(1, b"first")],
        )
        .await
        .expect("first publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.beta"),
            Some(0),
            &[make_event(2, b"other")],
        )
        .await
        .expect("interleaved publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(1),
            &[make_event(3, b"second")],
        )
        .await
        .expect("subject guard should use latest subject sequence");

        let stale = append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(1),
            &[make_event(4, b"stale")],
        )
        .await
        .expect_err("stale subject sequence should fail");

        assert!(matches!(stale, StreamStoreError::WrongExpectedVersion));
        assert_eq!(js.last_subject_sequence("test.events.alpha"), 3);
        assert_eq!(js.last_subject_sequence("test.events.beta"), 2);
    }

    #[tokio::test]
    async fn append_stream_maps_wrong_last_sequence_to_optimistic_concurrency() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_ack_error(PublishErrorKind::WrongLastSequence);

        let error = append_stream(&js, make_subject("test.events"), Some(7), &[make_event(1, b"x")])
            .await
            .expect_err("publish should fail");

        assert!(matches!(error, StreamStoreError::WrongExpectedVersion));
    }

    #[tokio::test]
    async fn append_stream_maps_synchronous_publish_error_to_publish_error() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_publish_error(PublishErrorKind::TimedOut);

        let error = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"x")])
            .await
            .expect_err("publish should fail");

        assert!(matches!(error, StreamStoreError::Publish(_)));
    }

    #[tokio::test]
    async fn append_stream_rejects_duplicate_event_id_via_ack() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_duplicate();

        let error = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"x")])
            .await
            .expect_err("duplicate publish should fail");

        assert!(matches!(error, StreamStoreError::Publish(_)));
    }

    #[tokio::test]
    async fn append_stream_maps_ack_error_to_publish_error() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_ack_error(PublishErrorKind::TimedOut);

        let error = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"x")])
            .await
            .expect_err("ack error should propagate");

        assert!(matches!(error, StreamStoreError::Publish(_)));
    }

    #[tokio::test]
    async fn read_stream_range_skips_no_message_found_gaps() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message(1, make_stream_message("test.events", 1, Uuid::from_u128(1)));
        // sequence 2 missing → mock returns NoMessageFound by default
        factory.add_raw_message(3, make_stream_message("test.events", 3, Uuid::from_u128(3)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = read_stream_range(&stream, 1, 3).await.expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_position, StreamPosition::try_new(1).unwrap());
        assert_eq!(events[1].stream_position, StreamPosition::try_new(3).unwrap());
        assert_eq!(factory.raw_message_calls(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn read_subject_stream_filters_by_subject() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message(1, make_stream_message("test.events.a", 1, Uuid::from_u128(1)));
        factory.add_raw_message(2, make_stream_message("test.events.b", 2, Uuid::from_u128(2)));
        factory.add_raw_message(3, make_stream_message("test.events.a", 3, Uuid::from_u128(3)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = super::read_subject_stream(&stream, "test.events.a", "test.events.a", 1, 3)
            .await
            .expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_id, "test.events.a");
        assert_eq!(events[1].stream_id, "test.events.a");
    }

    #[tokio::test]
    async fn read_subject_stream_uses_caller_stream_id() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message(1, make_stream_message("test.events.a", 1, Uuid::from_u128(1)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = super::read_subject_stream(&stream, "a", "test.events.a", 1, 1)
            .await
            .expect("read should succeed");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stream_id, "a");
    }

    #[tokio::test]
    async fn read_subject_stream_observes_semantic_publish_mock() {
        let js = MockJetStreamPublishMessage::new();
        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(0),
            &[make_event(1, b"alpha-one")],
        )
        .await
        .expect("alpha publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.beta"),
            Some(0),
            &[make_event(2, b"beta-one")],
        )
        .await
        .expect("beta publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(1),
            &[make_event(3, b"alpha-two")],
        )
        .await
        .expect("alpha second publish should succeed");

        let events = super::read_subject_stream(&js, "alpha", "test.events.alpha", 1, 3)
            .await
            .expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_id, "alpha");
        assert_eq!(events[0].stream_position, StreamPosition::try_new(1).unwrap());
        assert_eq!(events[1].stream_position, StreamPosition::try_new(3).unwrap());
    }

    #[tokio::test]
    async fn read_stream_range_returns_empty_for_invalid_bounds() {
        let factory = MockJetStreamConsumerFactory::new();
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        assert!(read_stream_range(&stream, 0, 1).await.unwrap().is_empty());
        assert!(read_stream_range(&stream, 1, 0).await.unwrap().is_empty());
        assert!(read_stream_range(&stream, 5, 3).await.unwrap().is_empty());
        assert!(factory.raw_message_calls().is_empty());
    }

    #[tokio::test]
    async fn read_stream_range_propagates_non_no_message_errors() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message_error(1, LastRawMessageErrorKind::Other);
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let error = read_stream_range(&stream, 1, 1)
            .await
            .expect_err("error should propagate");

        assert!(matches!(error, StreamStoreError::Read(_)));
    }

    #[tokio::test]
    async fn read_stream_uses_info_last_sequence_as_upper_bound() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.set_info(make_stream_info(2));
        factory.add_raw_message(1, make_stream_message("test.events", 1, Uuid::from_u128(1)));
        factory.add_raw_message(2, make_stream_message("test.events", 2, Uuid::from_u128(2)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = read_stream(&stream, 1).await.expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(factory.raw_message_calls(), vec![1, 2]);
    }

    #[tokio::test]
    async fn subject_current_position_returns_none_when_no_message_found() {
        let factory = MockJetStreamConsumerFactory::new();
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let current = subject_current_position(&stream, &subject).await.unwrap();

        assert_eq!(current, None);
    }

    #[tokio::test]
    async fn subject_current_position_returns_latest_sequence() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message(async_nats::jetstream::message::StreamMessage {
            subject: "test.events.backup".into(),
            sequence: 17,
            headers: async_nats::HeaderMap::new(),
            payload: bytes::Bytes::from_static(b"{}"),
            time: time::OffsetDateTime::UNIX_EPOCH,
        });
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let current = subject_current_position(&stream, &subject).await.unwrap();

        assert_eq!(current, Some(StreamPosition::try_new(17).unwrap()));
    }

    #[tokio::test]
    async fn subject_current_position_propagates_non_no_message_errors() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message_error(LastRawMessageError::new(LastRawMessageErrorKind::Other));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let error = subject_current_position(&stream, &subject).await.unwrap_err();

        assert!(matches!(error, StreamStoreError::Read(_)));
    }
