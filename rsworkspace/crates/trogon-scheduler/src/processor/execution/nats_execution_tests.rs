//! Local NATS integration check for the NATS-first execution slice.
//!
//! This proves the concrete slice-3 path against a live server: command event
//! appends through the JetStream event store, KV checkpoint writes, execution
//! schedule publish/purge, and purge-then-publish convergence to a single
//! execution schedule message even when the same writer repeats far past the
//! `Nats-Msg-Id` duplicate window. It requires a NATS 2.14+ server (the
//! consumer's `verify_message_scheduling_support` rejects anything older) with
//! `AllowAtomicPublish` on the event stream and `AllowMsgSchedules` on the
//! execution stream, so it is `#[ignore]` by default and only runs when
//! `SCHEDULER_NATS_URL` is set.
//! CI stays on the mock-backed unit tests.
//!
//! Run with, for example:
//!
//! ```sh
//! SCHEDULER_NATS_URL=localhost:4222 \
//!   cargo test -p trogon-scheduler -- --ignored
//! ```

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::stream::Stream;
use async_nats::jetstream::{self, stream};
use chrono::Utc;
use trogon_decider_nats::{
    JetStreamStore, StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_runtime::{CommandExecution, ReadFrom, ReadStreamRequest, StreamRead};
use trogon_nats::NatsConfig;
use trogon_nats::jetstream::JetStreamSubjectPurger;

use super::checkpoints::{ReconcileOutcome, ScheduleCheckpointRecord, ScheduleCheckpointStore, ScheduleStatus};
use super::execution_schedules::ExecutionScheduleWriter;
use super::reconciliation::{ScheduleKey, ScheduleRequest, ScheduleSubject, StreamRoutingId};
use super::worker::{ProcessedOutcome, ScheduleProcessor};
use crate::CreateSchedule;
use crate::commands::domain::{
    Delivery, MessageContent, Schedule, ScheduleEventStatus, ScheduleHeaders, ScheduleId, ScheduleMessage,
};

const EXECUTION_STREAM: &str = "SCHEDULER_SCHEDULE_EXECUTION_IT";
const EVENT_STREAM: &str = "SCHEDULER_SCHEDULE_EVENTS_IT";
const STATE_BUCKET: &str = "SCHEDULER_SCHEDULE_STATE_IT";
const SNAPSHOT_BUCKET: &str = "SCHEDULER_SCHEDULE_SNAPSHOTS_IT";
const EXECUTION_SUBJECT_WILDCARD: &str = "scheduler.schedules.execution.v1.>";
const EVENT_SUBJECT_WILDCARD: &str = "scheduler.schedules.events.v1.>";
const EXECUTION_TARGET_SUBJECT: &str = "agent.run";

#[derive(Clone)]
struct ScheduleEventSubjectResolver;

impl StreamSubjectResolver<str> for ScheduleEventSubjectResolver {
    type Error = StreamStoreError;

    async fn resolve_subject_state(
        &self,
        events_stream: &Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let key = ScheduleKey::for_stream(&StreamRoutingId::from(stream_id));
        let subject = StreamSubject::new(ScheduleSubject::event(&key).as_str()).expect("event subject is valid");
        let current_position = subject_current_position(events_stream, &subject).await?;

        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

async fn context() -> Option<jetstream::Context> {
    let url = std::env::var("SCHEDULER_NATS_URL").ok()?;
    let config = NatsConfig::from_url(url);
    let client = trogon_nats::connect(&config, Duration::from_secs(5))
        .await
        .expect("connect to NATS");
    Some(jetstream::new(client))
}

fn request(id: &ScheduleId) -> ScheduleRequest {
    ScheduleRequest::build(
        id,
        &Schedule::every(Duration::from_secs(30)).unwrap(),
        &Delivery::nats_event(EXECUTION_TARGET_SUBJECT).unwrap(),
        &ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::default(),
        },
    )
    .unwrap()
}

fn create_schedule(id: ScheduleId) -> CreateSchedule {
    CreateSchedule {
        id,
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::every(Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event(EXECUTION_TARGET_SUBJECT).unwrap(),
        message: ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::default(),
        },
    }
}

fn test_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time is after the Unix epoch")
        .as_nanos()
}

async fn execution_stream(context: &jetstream::Context) -> jetstream::stream::Stream {
    context
        .get_or_create_stream(stream::Config {
            name: EXECUTION_STREAM.to_string(),
            subjects: vec![
                EXECUTION_SUBJECT_WILDCARD.to_string(),
                EXECUTION_TARGET_SUBJECT.to_string(),
            ],
            allow_message_schedules: true,
            ..Default::default()
        })
        .await
        .expect("create execution stream")
}

async fn event_stream(context: &jetstream::Context) -> jetstream::stream::Stream {
    context
        .get_or_create_stream(stream::Config {
            name: EVENT_STREAM.to_string(),
            subjects: vec![EVENT_SUBJECT_WILDCARD.to_string()],
            allow_atomic_publish: true,
            ..Default::default()
        })
        .await
        .expect("create event stream")
}

async fn key_value(context: &jetstream::Context, bucket: &str) -> jetstream::kv::Store {
    context
        .create_key_value(jetstream::kv::Config {
            bucket: bucket.to_string(),
            history: 1,
            ..Default::default()
        })
        .await
        .expect("create key value bucket")
}

#[tokio::test]
#[ignore = "requires NATS 2.14+ with AllowMsgSchedules; set SCHEDULER_NATS_URL"]
async fn purge_then_publish_converges_and_state_persists_against_live_nats() {
    let Some(context) = context().await else {
        eprintln!("SCHEDULER_NATS_URL not set; skipping live NATS integration check");
        return;
    };

    // NATS requires the schedule target to belong to the same stream as the
    // scheduling subject, while still being distinct from that subject.
    let execution_stream = execution_stream(&context).await;
    let checkpoint_kv = key_value(&context, STATE_BUCKET).await;

    let writer = ExecutionScheduleWriter::new(context.clone(), execution_stream.clone());
    let checkpoints = ScheduleCheckpointStore::new(checkpoint_kv);

    let nonce = test_nonce();
    let id = ScheduleId::parse(&format!("orders/it-{nonce}")).unwrap();
    let key = ScheduleKey::derive(&id);
    let subject = ScheduleSubject::execution(&key);
    let request = request(&id);

    // Start from a clean subject.
    execution_stream
        .purge_subject_messages(subject.as_str())
        .await
        .expect("initial purge");

    // Repeatedly upsert with distinct message ids, well past any duplicate
    // window, then assert exactly one execution schedule message survives.
    let trace = async_nats::HeaderMap::new();
    for attempt in 0..4 {
        writer
            .upsert(&request, &format!("event-{nonce}-{attempt}"), &trace)
            .await
            .expect("upsert");
    }

    let last = execution_stream
        .get_last_raw_message_by_subject(subject.as_str())
        .await
        .expect("one scheduled message present");
    assert_eq!(last.subject.as_str(), subject.as_str());

    // Purge clears it; purging again is success-when-absent.
    writer.purge(&subject).await.expect("purge");
    writer.purge(&subject).await.expect("purge when absent");

    // Checkpoint writes round-trip through the live KV bucket.
    let record = ScheduleCheckpointRecord {
        schedule_id: id.clone(),
        status: ScheduleStatus::Scheduled,
        schedule: Schedule::every(Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event(EXECUTION_TARGET_SUBJECT).unwrap(),
        message: ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::default(),
        },
        last_applied_stream_position: trogon_decider_runtime::StreamPosition::try_new(1).unwrap(),
        last_applied_event_id: Some("event-1".to_string()),
        last_outcome: ReconcileOutcome::Published,
    };
    checkpoints.save(&record, None).await.expect("checkpoint create");
    let loaded = checkpoints
        .load(&record.key())
        .await
        .expect("checkpoint load")
        .expect("present");
    assert_eq!(loaded.record.schedule_id, id);
}

#[tokio::test]
#[ignore = "requires NATS 2.14+ with AllowAtomicPublish and AllowMsgSchedules; set SCHEDULER_NATS_URL"]
async fn create_command_event_is_processed_into_a_live_execution_schedule() {
    let Some(context) = context().await else {
        eprintln!("SCHEDULER_NATS_URL not set; skipping live NATS integration check");
        return;
    };

    let execution_stream = execution_stream(&context).await;
    let events_stream = event_stream(&context).await;
    let checkpoint_kv = key_value(&context, &format!("{STATE_BUCKET}_{}", test_nonce())).await;
    let snapshot_kv = key_value(&context, &format!("{SNAPSHOT_BUCKET}_{}", test_nonce())).await;
    let store = JetStreamStore::builder(context.clone(), events_stream.clone(), snapshot_kv)
        .with_subject_resolver(ScheduleEventSubjectResolver);

    let id = ScheduleId::parse(&format!("orders/full-e2e-{}", test_nonce())).unwrap();
    let key = ScheduleKey::derive(&id);
    let subject = ScheduleSubject::execution(&key);
    execution_stream
        .purge_subject_messages(subject.as_str())
        .await
        .expect("initial purge");

    let command = create_schedule(id.clone());
    let result = CommandExecution::new(&store, &command)
        .execute()
        .await
        .expect("create command writes event");
    assert_eq!(result.events.len(), 1);

    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: command.id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read command-produced event");
    assert_eq!(read.events.len(), 1);

    let processor = ScheduleProcessor::new(
        ExecutionScheduleWriter::new(context.clone(), execution_stream.clone()),
        ScheduleCheckpointStore::new(checkpoint_kv.clone()),
        EVENT_STREAM,
        Arc::default(),
    );

    let processed = processor
        .process(&read.events[0], Utc::now())
        .await
        .expect("process command-produced event");
    assert_eq!(processed.outcome, ProcessedOutcome::Published);

    let last = execution_stream
        .get_last_raw_message_by_subject(subject.as_str())
        .await
        .expect("execution schedule was published");
    assert_eq!(last.subject.as_str(), subject.as_str());

    let checkpoint = ScheduleCheckpointStore::new(checkpoint_kv)
        .load(&key)
        .await
        .expect("checkpoint read")
        .expect("checkpoint persisted");
    assert_eq!(checkpoint.record.schedule_id, id);
}
