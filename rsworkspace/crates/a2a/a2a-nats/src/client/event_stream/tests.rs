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

/// The wire body of a task event: a JSON-RPC success response repeating the
/// request id, with the `StreamResponse` as its result.
fn event_body(id: &str, event: &StreamResponse) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": event,
    }))
    .unwrap()
}

fn nats_msg_with_reply(payload: Vec<u8>, reply: Option<&str>) -> async_nats::Message {
    async_nats::Message {
        subject: "a2a.v1.tasks.t1.events".into(),
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
    let mut stream = build_event_stream(consumer, last_seq.clone(), None);

    let payload = event_body("req-1", &make_status_event("task-1"));
    tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, None))))
        .unwrap();
    drop(tx);

    let item = stream.next().await;
    assert!(item.is_some());
    assert!(item.unwrap().is_ok());
}

fn event_stamped_for(req_id: &str) -> async_nats::Message {
    let payload = event_body(req_id, &make_status_event(req_id));
    let mut message = nats_msg_with_reply(payload, Some(&ack_reply(1)));
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(crate::constants::REQ_ID_HEADER, req_id);
    message.headers = Some(headers);
    message
}

#[tokio::test]
async fn stream_drops_events_stamped_for_another_subscription() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, Some(ReqId::from_test("req-mine")));

    for req_id in ["req-theirs", "req-mine"] {
        tx.unbounded_send(Ok(MockJsMessage::new(event_stamped_for(req_id))))
            .unwrap();
    }
    drop(tx);

    let first = stream.next().await.expect("one event survives").unwrap();
    match first {
        StreamResponse::StatusUpdate(event) => assert_eq!(event.task_id, "req-mine"),
        other => panic!("expected a status update, got {other:?}"),
    }
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn dropping_a_foreign_event_survives_a_failed_ack() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, Some(ReqId::from_test("req-mine")));

    tx.unbounded_send(Ok(MockJsMessage::with_failing_signals(event_stamped_for("req-theirs"))))
        .unwrap();
    tx.unbounded_send(Ok(MockJsMessage::new(event_stamped_for("req-mine"))))
        .unwrap();
    drop(tx);

    assert!(
        stream
            .next()
            .await
            .expect("delivery continues past the failed ack")
            .is_ok()
    );
}

#[tokio::test]
async fn stream_keeps_every_event_when_no_subscription_id_is_given() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, None);

    tx.unbounded_send(Ok(MockJsMessage::new(event_stamped_for("req-theirs"))))
        .unwrap();
    drop(tx);

    assert!(stream.next().await.expect("a resume replays everything").is_ok());
}

#[tokio::test]
async fn stream_closes_when_sender_dropped() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, None);
    drop(tx);

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn stream_yields_error_on_bad_payload() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, None);

    tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(b"not json".to_vec(), None))))
        .unwrap();
    drop(tx);

    let item = stream.next().await;
    assert!(matches!(item, Some(Err(ClientError::Codec(_)))));
}

#[tokio::test]
async fn stream_still_reads_an_event_an_older_agent_published() {
    // `A2A_EVENTS` retains by limits and a rolling upgrade runs both agent
    // releases at once, so a bare `StreamResponse` is a body this reader meets
    // rather than a malformed one. Refusing it would end the stream mid-task.
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, None);

    let payload = serde_json::to_vec(&make_status_event("task-1")).unwrap();
    tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, None))))
        .unwrap();
    drop(tx);

    let event = stream.next().await.expect("a pre-envelope event").unwrap();
    assert_eq!(event, make_status_event("task-1"));
}

#[tokio::test]
async fn stream_maps_a_jsonrpc_error_event_to_a_typed_error() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, None);

    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-1",
        "error": { "code": crate::constants::TASK_NOT_FOUND, "message": "Task not found" },
    }))
    .unwrap();
    tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, None))))
        .unwrap();
    drop(tx);

    assert!(matches!(stream.next().await, Some(Err(ClientError::TaskNotFound))));
}

#[tokio::test]
async fn stream_yields_error_on_consumer_stream_error() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq, None);

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
    let mut stream = build_event_stream(consumer, last_seq, None);

    let item = stream.next().await;
    assert!(matches!(item, Some(Err(ClientError::ConsumerSetup(_)))));
}

#[tokio::test]
async fn last_seq_advances_only_after_successful_downstream_send() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let mut stream = build_event_stream(consumer, last_seq.clone(), None);

    let payload = event_body("req-1", &make_status_event("task-1"));
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
    let mut stream = build_event_stream(consumer, last_seq.clone(), None);

    let payload = event_body("req-1", &make_status_event("task-1"));
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

    let (tx, receiver) = mpsc::channel::<Result<StreamResponse, ClientError>>(EVENT_STREAM_QUEUE_CAPACITY);
    drop(receiver); // Channel closed before pull_loop sees the message.

    let payload = event_body("req-1", &make_status_event("task-1"));
    let reply = ack_reply(42);
    msg_tx
        .unbounded_send(Ok(MockJsMessage::new(nats_msg_with_reply(payload, Some(&reply)))))
        .unwrap();
    drop(msg_tx);

    pull_loop(consumer, tx, last_seq.clone(), None).await;

    // Send failed, so the loop returned without advancing the cursor or acking.
    assert_eq!(*through_poison(last_seq.lock()), 0);
}

#[tokio::test]
async fn dropping_stream_aborts_pull_loop() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let stream = build_event_stream(consumer, last_seq, None);

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
    let stream = build_event_stream(consumer, last_seq.clone(), None);
    drop(tx);
    drop(stream);
    assert_eq!(*through_poison(last_seq.lock()), 0);
}

#[tokio::test]
async fn last_seq_is_accessible_on_stream() {
    let (consumer, tx) = MockJetStreamConsumer::new();
    let last_seq = Arc::new(Mutex::new(0u64));
    let stream = build_event_stream(consumer, last_seq.clone(), None);
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

#[tokio::test]
async fn empty_event_stream_is_already_finished() {
    let mut stream = empty_event_stream();
    assert_eq!(stream.last_seq(), 0);
    assert!(stream.next().await.is_none(), "no events ever follow a bare Message");
}
