use async_nats::{
    HeaderMap,
    header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_ID},
    jetstream,
    jetstream::{context::PublishErrorKind, stream::LastRawMessageError, stream::LastRawMessageErrorKind},
};
use bytes::Bytes;
use trogon_nats::jetstream::JetStreamGetStream;
use trogon_nats::jetstream::mocks::{MockJetStreamConsumerFactory, MockJetStreamPublishMessage};
use trogon_nats::test_support::JetStreamTestServer;
use uuid::Uuid;

use trogon_decider_runtime::{Event, EventId, Headers, StreamPosition};

use super::replay::is_empty_replay_range;
use super::{
    NATS_BATCH_COMMIT, NATS_BATCH_ID, NATS_BATCH_SEQUENCE, StreamStoreError, StreamSubject, TROGON_EVENT_HEADER_PREFIX,
    TROGON_EVENT_TYPE, append_stream, build_publish_message, headers_from_nats_headers, read_stream, read_stream_range,
    subject_current_position,
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
    assert!(is_empty_replay_range(0, 1));
    assert!(is_empty_replay_range(1, 0));
    assert!(is_empty_replay_range(2, 1));
    assert!(!is_empty_replay_range(1, 1));
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

// Ordered-consumer delivery metadata requires a real JetStream server and cannot be represented by the mocks.
async fn create_events_stream(
    js: &jetstream::Context,
    name: &str,
    subject_wildcard: &str,
) -> jetstream::stream::Stream {
    js.create_stream(jetstream::stream::Config {
        name: name.to_string(),
        subjects: vec![subject_wildcard.to_string()],
        allow_atomic_publish: true,
        ..Default::default()
    })
    .await
    .expect("create test events stream")
}

async fn publish_noise(js: &jetstream::Context, subject: &str, count: usize) {
    for index in 0..count {
        js.publish(subject.to_string(), Bytes::from(format!("noise-{index}")))
            .await
            .expect("send noise publish")
            .await
            .expect("ack noise publish");
    }
}

#[cfg(not(coverage))]
#[tokio::test]
async fn read_subject_stream_filters_by_subject_amid_heavy_foreign_traffic() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "FILTER_TEST", "filter.test.>").await;

    let alpha_subject = "filter.test.alpha";
    let noise_subject = "filter.test.noise";

    let mut alpha_positions = Vec::new();
    for index in 0..3u128 {
        publish_noise(&js, noise_subject, 25).await;
        let position = append_stream(
            &js,
            make_subject(alpha_subject),
            None,
            &[make_event(100 + index, format!("alpha-{index}").as_bytes())],
        )
        .await
        .expect("publish alpha event");
        alpha_positions.push(position);
    }
    publish_noise(&js, noise_subject, 25).await;

    let to_sequence = subject_current_position(&stream, &make_subject(alpha_subject))
        .await
        .expect("read alpha current position")
        .expect("alpha subject should have a current position")
        .as_u64();

    let events = super::read_subject_stream(&stream, "alpha", alpha_subject, 1, to_sequence)
        .await
        .expect("replay should succeed");

    assert_eq!(events.len(), 3);
    for (event, position) in events.iter().zip(alpha_positions.iter()) {
        assert_eq!(event.stream_id, "alpha");
        assert_eq!(event.stream_position, *position);
    }
}

#[tokio::test]
async fn read_subject_stream_respects_from_and_to_sequence_bounds() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "BOUNDS_TEST", "bounds.test.>").await;

    let subject = "bounds.test.alpha";
    let mut positions = Vec::new();
    for index in 0..5u128 {
        let position = append_stream(
            &js,
            make_subject(subject),
            None,
            &[make_event(index, format!("event-{index}").as_bytes())],
        )
        .await
        .expect("publish event");
        positions.push(position);
    }

    let middle = super::read_subject_stream(&stream, "alpha", subject, positions[1].as_u64(), positions[3].as_u64())
        .await
        .expect("replay should succeed");

    assert_eq!(middle.len(), 3);
    assert_eq!(middle[0].stream_position, positions[1]);
    assert_eq!(middle[2].stream_position, positions[3]);

    let inverted = super::read_subject_stream(&stream, "alpha", subject, positions[3].as_u64(), positions[1].as_u64())
        .await
        .expect("inverted range should not error");
    assert!(inverted.is_empty());
}

#[tokio::test]
async fn read_subject_stream_uses_caller_stream_id() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "STREAM_ID_TEST", "stream.id.test.>").await;

    let subject = "stream.id.test.alpha";
    let position = append_stream(&js, make_subject(subject), None, &[make_event(1, b"payload")])
        .await
        .expect("publish event");

    let events = super::read_subject_stream(&stream, "custom-stream-id", subject, 1, position.as_u64())
        .await
        .expect("replay should succeed");

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].stream_id, "custom-stream-id");
}

#[tokio::test]
async fn read_stream_range_reads_all_subjects_in_sequence_order() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "ALL_SUBJECTS_TEST", "all.subjects.test.>").await;

    let alpha = "all.subjects.test.alpha";
    let beta = "all.subjects.test.beta";
    let mut expected_subjects = Vec::new();
    for index in 0..4u128 {
        let subject = if index % 2 == 0 { alpha } else { beta };
        append_stream(
            &js,
            make_subject(subject),
            None,
            &[make_event(index, format!("event-{index}").as_bytes())],
        )
        .await
        .expect("publish event");
        expected_subjects.push(subject.to_string());
    }

    let events = read_stream_range(&stream, 1, 4).await.expect("replay should succeed");

    assert_eq!(events.len(), 4);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            event.stream_position,
            StreamPosition::try_new((index as u64) + 1).unwrap()
        );
        assert_eq!(event.stream_id, expected_subjects[index]);
    }
}

#[tokio::test]
async fn read_stream_reads_up_to_current_stream_tail() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "TAIL_TEST", "tail.test.>").await;

    let subject = "tail.test.alpha";
    for index in 0..3u128 {
        append_stream(
            &js,
            make_subject(subject),
            None,
            &[make_event(index, format!("event-{index}").as_bytes())],
        )
        .await
        .expect("publish event");
    }

    let events = read_stream(&stream, 1).await.expect("replay should succeed");

    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn read_stream_range_propagates_fatal_errors_without_retrying() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "DELETED_STREAM_TEST", "deleted.stream.test.>").await;
    js.delete_stream("DELETED_STREAM_TEST")
        .await
        .expect("delete underlying stream");

    let error = read_stream_range(&stream, 1, 1)
        .await
        .expect_err("replay against a deleted stream should fail");

    assert!(matches!(error, StreamStoreError::Read(_)));
}

#[tokio::test]
async fn read_subject_stream_resumes_from_next_sequence_after_partial_progress() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "RESUME_TEST", "resume.test.>").await;

    let subject = "resume.test.alpha";
    let noise_subject = "resume.test.noise";
    let mut positions = Vec::new();
    for index in 0..5u128 {
        publish_noise(&js, noise_subject, 5).await;
        let position = append_stream(
            &js,
            make_subject(subject),
            None,
            &[make_event(index, format!("event-{index}").as_bytes())],
        )
        .await
        .expect("publish event");
        positions.push(position);
    }
    let to_sequence = positions.last().unwrap().as_u64();

    let full = super::read_subject_stream(&stream, "alpha", subject, 1, to_sequence)
        .await
        .expect("full replay should succeed");
    assert_eq!(full.len(), 5);

    // Simulates what the outer retry loop does after a transient failure: it
    // recreates the consumer starting right after the last event it already
    // produced, instead of restarting the whole range from scratch.
    let checkpoint = full[2].stream_position.as_u64();
    let resumed = super::read_subject_stream(&stream, "alpha", subject, checkpoint + 1, to_sequence)
        .await
        .expect("resumed replay should succeed");

    assert_eq!(resumed.len(), 2);
    assert_eq!(resumed[0].event.id, full[3].event.id);
    assert_eq!(resumed[1].event.id, full[4].event.id);
    assert_eq!(resumed[0].stream_position, full[3].stream_position);
    assert_eq!(resumed[1].stream_position, full[4].stream_position);
}
