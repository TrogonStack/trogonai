    use a2a::event::TaskStatusUpdateEvent;
    use a2a::types::{TaskState, TaskStatus};
    use bytes::Bytes;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJsMessage};

    use super::*;

    fn make_status_event(task_id: &str) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        })
    }

    fn nats_msg_with_reply(payload: Vec<u8>, reply: Option<&str>) -> async_nats::Message {
        async_nats::Message {
            subject: "a2a.tasks.t1.events.r1".into(),
            reply: reply.map(|s| s.into()),
            payload: Bytes::from(payload),
            headers: None,
            status: None,
            description: None,
            length: 0,
        }
    }

    fn ack_reply(stream_seq: u64) -> String {
        format!("$JS.ACK.A2A_EVENTS.consumer-1.1.{stream_seq}.1.1700000000000000000.0")
    }

    #[tokio::test]
    async fn stream_yields_deserialized_events() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq.clone());

        let event = make_status_event("task-1");
        let payload = serde_json::to_vec(&event).unwrap();
        tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, None))))
            .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(item.is_some());
        assert!(item.unwrap().is_ok());
    }

    #[tokio::test]
    async fn stream_closes_when_sender_dropped() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);
        drop(tx);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_yields_error_on_bad_payload() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);

        tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(b"not json".to_vec(), None))))
            .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(matches!(item, Some(Err(ClientError::Deserialize(_)))));
    }

    #[tokio::test]
    async fn stream_yields_error_on_consumer_stream_error() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);

        tx.unbounded_send(Err(trogon_nats::mocks::MockError("boom".to_string())))
            .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(matches!(item, Some(Err(ClientError::JetStream(_)))));
    }

    #[tokio::test]
    async fn consumer_setup_failure_emits_consumer_setup_error() {
        let consumer = MockJetStreamConsumer::failing();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);

        let item = stream.next().await;
        assert!(matches!(item, Some(Err(ClientError::ConsumerSetup(_)))));
    }

    #[tokio::test]
    async fn last_seq_advances_only_after_successful_downstream_send() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq.clone());

        let event = make_status_event("task-1");
        let payload = serde_json::to_vec(&event).unwrap();
        let reply = ack_reply(7);
        tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, Some(&reply)))))
            .unwrap();
        drop(tx);

        let _ = stream.next().await;
        let _ = stream.next().await;
        assert_eq!(stream.last_seq(), 7);
    }

    #[tokio::test]
    async fn ack_failure_does_not_stop_delivery() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq.clone());

        let event = make_status_event("task-1");
        let payload = serde_json::to_vec(&event).unwrap();
        let reply = ack_reply(3);
        tx.unbounded_send(Ok(MockJsMessage::with_failing_signals(nats_msg_with_reply(
            payload,
            Some(&reply),
        ))))
        .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(item.is_some());
        assert!(item.unwrap().is_ok());
        // Sequence still advanced even though the ack failed — the receiver got the event.
        let _ = stream.next().await;
        assert_eq!(stream.last_seq(), 3);
    }

    #[tokio::test]
    async fn pull_loop_returns_early_when_receiver_dropped() {
        let (consumer, msg_tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));

        let (tx, receiver) = mpsc::unbounded::<Result<StreamResponse, ClientError>>();
        drop(receiver); // Channel closed before pull_loop sees the message.

        let event = make_status_event("task-1");
        let payload = serde_json::to_vec(&event).unwrap();
        let reply = ack_reply(42);
        msg_tx
            .unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, Some(&reply)))))
            .unwrap();
        drop(msg_tx);

        pull_loop(consumer, tx, last_seq.clone()).await;

        // Send failed, so the loop returned without advancing the cursor or acking.
        assert_eq!(*through_poison(last_seq.lock()), 0);
    }

    #[tokio::test]
    async fn dropping_stream_aborts_pull_loop() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let stream = build_event_stream(consumer, last_seq);

        // Drop the stream while the consumer is still alive and idle — the
        // spawned task should be aborted rather than sitting on msgs.next().
        drop(stream);

        // Sending after drop should not block forever; receiver is gone and the
        // task is aborted, so the channel is closed.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(tx.is_closed());
    }

    #[tokio::test]
    async fn last_seq_starts_at_zero() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let stream = build_event_stream(consumer, last_seq.clone());
        drop(tx);
        drop(stream);
        assert_eq!(*through_poison(last_seq.lock()), 0);
    }

    #[tokio::test]
    async fn last_seq_is_accessible_on_stream() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let stream = build_event_stream(consumer, last_seq.clone());
        drop(tx);
        assert_eq!(stream.last_seq(), 0);
    }

    #[test]
    fn stream_sequence_from_reply_parses_well_formed_ack_subject() {
        assert_eq!(stream_sequence_from_reply(Some(&ack_reply(99))), Some(99));
    }

    #[test]
    fn stream_sequence_from_reply_returns_none_for_missing_reply() {
        assert_eq!(stream_sequence_from_reply(None), None);
    }

    #[test]
    fn stream_sequence_from_reply_returns_none_for_non_jetstream_reply() {
        assert_eq!(stream_sequence_from_reply(Some("INBOX.abc")), None);
    }

    #[test]
    fn stream_sequence_from_reply_returns_none_for_short_subject() {
        assert_eq!(stream_sequence_from_reply(Some("$JS.ACK.stream.consumer")), None);
    }

    #[test]
    fn stream_sequence_from_reply_returns_none_for_non_numeric_seq() {
        assert_eq!(
            stream_sequence_from_reply(Some("$JS.ACK.A2A_EVENTS.c.1.NOT_A_NUMBER.1.0.0")),
            None
        );
    }

    #[test]
    fn through_poison_recovers_from_poisoned_lock() {
        let m = Arc::new(Mutex::new(0u64));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("intentional");
        })
        .join();
        assert!(m.lock().is_err());
        let mut g = through_poison(m.lock());
        *g = 5;
        drop(g);
        assert_eq!(*through_poison(m.lock()), 5);
    }
