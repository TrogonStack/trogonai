use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio::sync::Mutex;
use trogon_decider_runtime::{Event, EventId, Headers, StreamEvent};
use trogon_nats::test_support::JetStreamTestServer;
use uuid::Uuid;

use crate::append_stream;
use crate::stream_store::{ReadStreamError, StreamStoreError, StreamSubject};

use super::{CatchUpError, CheckpointSequence, ProjectionApply, ProjectionCheckpointStore, Projector};

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

struct FakeCheckpointStore {
    checkpoint: Mutex<CheckpointSequence>,
}

impl FakeCheckpointStore {
    fn new(initial: CheckpointSequence) -> Self {
        Self {
            checkpoint: Mutex::new(initial),
        }
    }

    async fn current(&self) -> CheckpointSequence {
        *self.checkpoint.lock().await
    }
}

impl ProjectionCheckpointStore for FakeCheckpointStore {
    type Error = std::convert::Infallible;

    async fn load(&self) -> Result<CheckpointSequence, Self::Error> {
        Ok(*self.checkpoint.lock().await)
    }

    async fn save(&self, checkpoint: CheckpointSequence) -> Result<(), Self::Error> {
        *self.checkpoint.lock().await = checkpoint;
        Ok(())
    }
}

#[derive(Clone, Default)]
struct RecordingApply {
    applied: Arc<Mutex<Vec<StreamEvent>>>,
}

impl RecordingApply {
    async fn applied(&self) -> Vec<StreamEvent> {
        self.applied.lock().await.clone()
    }
}

impl ProjectionApply for RecordingApply {
    type Error = std::convert::Infallible;

    async fn apply(&mut self, event: StreamEvent) -> Result<CheckpointSequence, Self::Error> {
        let checkpoint = CheckpointSequence::from(event.stream_position);
        self.applied.lock().await.push(event);
        Ok(checkpoint)
    }
}

#[test]
fn checkpoint_sequence_none_resumes_from_first_sequence() {
    assert_eq!(CheckpointSequence::NONE.next_from_sequence(), 1);
    assert!(CheckpointSequence::NONE.is_none());
}

#[test]
fn checkpoint_sequence_resumes_right_after_itself() {
    let checkpoint = CheckpointSequence::new(7);
    assert_eq!(checkpoint.next_from_sequence(), 8);
    assert!(!checkpoint.is_none());
}

#[tokio::test]
async fn catch_up_applies_all_events_from_zero() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_FROM_ZERO", "projector.from_zero.>").await;
    let subject = make_subject("projector.from_zero.alpha");
    for index in 0..4u128 {
        append_stream(&js, subject.clone(), None, &[make_event(index, b"payload")])
            .await
            .expect("publish event");
    }

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::NONE);
    let projector = Projector::new(stream, "projection", checkpoint_store);

    let outcome = projector
        .catch_up(RecordingApply::default())
        .await
        .expect("catch up should succeed");

    assert_eq!(outcome.events_applied, 4);
    assert_eq!(outcome.checkpoint, CheckpointSequence::new(4));
    assert!(outcome.reached_target);
    assert_eq!(projector.checkpoint_store().current().await, CheckpointSequence::new(4));
}

#[tokio::test]
async fn catch_up_resumes_from_existing_checkpoint() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_RESUME", "projector.resume.>").await;
    let subject = make_subject("projector.resume.alpha");
    let mut positions = Vec::new();
    for index in 0..5u128 {
        let position = append_stream(&js, subject.clone(), None, &[make_event(index, b"payload")])
            .await
            .expect("publish event");
        positions.push(position);
    }

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::from(positions[1]));
    let projector = Projector::new(stream, "projection", checkpoint_store);

    let apply = RecordingApply::default();
    let outcome = projector
        .catch_up(apply.clone())
        .await
        .expect("catch up should succeed");
    let applied = apply.applied().await;

    assert_eq!(outcome.events_applied, 3);
    assert_eq!(applied.len(), 3);
    assert_eq!(applied[0].stream_position, positions[2]);
    assert_eq!(outcome.checkpoint, CheckpointSequence::from(*positions.last().unwrap()));
}

#[tokio::test]
async fn catch_up_filters_by_subject() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_FILTER", "projector.filter.>").await;
    let alpha = make_subject("projector.filter.alpha");
    let beta = make_subject("projector.filter.beta");

    append_stream(&js, alpha.clone(), None, &[make_event(1, b"alpha-1")])
        .await
        .expect("publish alpha event");
    append_stream(&js, beta.clone(), None, &[make_event(2, b"beta-1")])
        .await
        .expect("publish beta event");
    append_stream(&js, alpha.clone(), None, &[make_event(3, b"alpha-2")])
        .await
        .expect("publish alpha event");

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::NONE);
    let projector = Projector::new(stream, "alpha-projection", checkpoint_store).with_filter_subject(alpha.to_string());

    let apply = RecordingApply::default();
    let outcome = projector
        .catch_up(apply.clone())
        .await
        .expect("catch up should succeed");
    let applied = apply.applied().await;

    assert_eq!(outcome.events_applied, 2);
    assert!(applied.iter().all(|event| event.stream_id == "alpha-projection"));
}

#[tokio::test]
async fn catch_up_filter_stops_at_last_matching_event_when_foreign_subject_owns_stream_tail() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_FOREIGN_TAIL", "projector.foreign_tail.>").await;
    let alpha = make_subject("projector.foreign_tail.alpha");
    let beta = make_subject("projector.foreign_tail.beta");

    let alpha_position = append_stream(&js, alpha.clone(), None, &[make_event(1, b"alpha")])
        .await
        .expect("publish alpha event");
    append_stream(&js, beta, None, &[make_event(2, b"beta")])
        .await
        .expect("publish beta event at physical stream tail");

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::NONE);
    let projector = Projector::new(stream, "alpha-projection", checkpoint_store).with_filter_subject(alpha.to_string());
    let apply = RecordingApply::default();

    let outcome = tokio::time::timeout(Duration::from_secs(2), projector.catch_up(apply.clone()))
        .await
        .expect("catch up should not wait for another matching publish")
        .expect("catch up should succeed");
    let applied = apply.applied().await;

    assert_eq!(outcome.events_applied, 1);
    assert_eq!(outcome.checkpoint, CheckpointSequence::from(alpha_position));
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].stream_position, alpha_position);
}

#[tokio::test]
async fn catch_up_wildcard_filter_stops_at_last_matching_event() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_WILDCARD_TAIL", "projector.wildcard_tail.>").await;
    let alpha = make_subject("projector.wildcard_tail.alpha.one");
    let beta = make_subject("projector.wildcard_tail.beta.one");

    let alpha_position = append_stream(&js, alpha, None, &[make_event(1, b"alpha")])
        .await
        .expect("publish alpha event");
    append_stream(&js, beta, None, &[make_event(2, b"beta")])
        .await
        .expect("publish beta event at physical stream tail");

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::NONE);
    let projector = Projector::new(stream, "alpha-projection", checkpoint_store)
        .with_filter_subject("projector.wildcard_tail.alpha.>");
    let apply = RecordingApply::default();

    let outcome = tokio::time::timeout(Duration::from_secs(2), projector.catch_up(apply.clone()))
        .await
        .expect("catch up should stop at the wildcard filter tail")
        .expect("catch up should succeed");
    let applied = apply.applied().await;

    assert_eq!(outcome.events_applied, 1);
    assert_eq!(outcome.checkpoint, CheckpointSequence::from(alpha_position));
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].stream_position, alpha_position);
}

#[tokio::test]
async fn catch_up_filter_with_no_matching_events_completes_without_replay() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_NO_MATCH", "projector.no_match.>").await;
    let beta = make_subject("projector.no_match.beta");

    append_stream(&js, beta, None, &[make_event(1, b"beta")])
        .await
        .expect("publish non-matching event");

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::NONE);
    let projector =
        Projector::new(stream, "alpha-projection", checkpoint_store).with_filter_subject("projector.no_match.alpha");

    let outcome = tokio::time::timeout(Duration::from_secs(2), projector.catch_up(RecordingApply::default()))
        .await
        .expect("catch up should not wait for a first matching publish")
        .expect("catch up should succeed");

    assert_eq!(outcome.events_applied, 0);
    assert_eq!(outcome.checkpoint, CheckpointSequence::NONE);
    assert!(outcome.reached_target);
}

#[tokio::test]
async fn catch_up_filter_propagates_target_query_failure() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_DELETED", "projector.deleted.>").await;
    js.delete_stream("PROJECTOR_DELETED")
        .await
        .expect("delete underlying stream");

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::NONE);
    let projector =
        Projector::new(stream, "projection", checkpoint_store).with_filter_subject("projector.deleted.alpha");

    let error = projector
        .catch_up(RecordingApply::default())
        .await
        .expect_err("querying a deleted stream should fail");

    assert!(matches!(
        error,
        CatchUpError::QueryTail(StreamStoreError::Read(ReadStreamError::ReadLatestSubjectMessage { .. }))
    ));
}

#[tokio::test]
async fn catch_up_reports_no_new_events_when_checkpoint_is_at_tail() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROJECTOR_AT_TAIL", "projector.at_tail.>").await;
    let subject = make_subject("projector.at_tail.alpha");
    let position = append_stream(&js, subject, None, &[make_event(1, b"payload")])
        .await
        .expect("publish event");

    let checkpoint_store = FakeCheckpointStore::new(CheckpointSequence::from(position));
    let projector = Projector::new(stream, "projection", checkpoint_store);

    let outcome = projector
        .catch_up(RecordingApply::default())
        .await
        .expect("catch up should succeed");

    assert_eq!(outcome.events_applied, 0);
    assert_eq!(outcome.checkpoint, CheckpointSequence::from(position));
    assert!(outcome.reached_target);
}
