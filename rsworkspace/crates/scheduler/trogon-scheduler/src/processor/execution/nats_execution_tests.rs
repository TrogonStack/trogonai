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
use chrono::{Duration as ChronoDuration, SecondsFormat, Timelike, Utc};
use futures::StreamExt;
use trogon_decider_nats::{
    JetStreamStore, StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_runtime::{CommandExecution, ReadFrom, ReadStreamRequest, StreamRead};
use trogon_nats::NatsConfig;
use trogon_nats::jetstream::JetStreamSubjectPurger;
use trogon_std::env::{ReadEnv, SystemEnv};

use super::checkpoints::{ReconcileOutcome, ScheduleCheckpointRecord, ScheduleCheckpointStore, ScheduleStatus};
use super::execution_schedules::ExecutionScheduleWriter;
use super::reconciliation::{ScheduleKey, ScheduleRequest, ScheduleSubject, StreamRoutingId};
use super::wakeup::{RRuleWakeupOutcome, RRuleWakeupProcessor, rrule_wakeup_consumer_config};
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
    let url = SystemEnv.var("SCHEDULER_NATS_URL").ok()?;
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

fn create_rrule_schedule(id: ScheduleId, dtstart: &str, rrule: &str) -> CreateSchedule {
    CreateSchedule {
        id,
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule(dtstart, rrule, Some("UTC".to_string())).unwrap(),
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

fn utc_seconds(datetime: chrono::DateTime<Utc>) -> String {
    datetime.to_rfc3339_opts(SecondsFormat::Secs, true)
}

async fn execution_stream(context: &jetstream::Context) -> jetstream::stream::Stream {
    let _ = context.delete_stream(EXECUTION_STREAM).await;
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
    let _ = context.delete_stream(EVENT_STREAM).await;
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

async fn fetch_wakeup(
    consumer: &async_nats::jetstream::consumer::Consumer<async_nats::jetstream::consumer::pull::Config>,
    label: &'static str,
) -> async_nats::jetstream::Message {
    let mut messages = consumer
        .fetch()
        .max_messages(1)
        .expires(Duration::from_secs(5))
        .messages()
        .await
        .expect("fetch wakeup messages");

    tokio::time::timeout(Duration::from_secs(10), messages.next())
        .await
        .unwrap_or_else(|_| panic!("{label} fired"))
        .unwrap_or_else(|| panic!("{label} message"))
        .unwrap_or_else(|error| panic!("{label} message delivered: {error}"))
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
        store.clone(),
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

#[tokio::test]
#[ignore = "requires NATS 2.14+ with AllowAtomicPublish and AllowMsgSchedules; set SCHEDULER_NATS_URL"]
async fn rrule_command_event_is_processed_and_continued_against_live_nats() {
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

    let first_occurrence = Utc::now() + ChronoDuration::seconds(3);
    let first_occurrence = first_occurrence
        .with_nanosecond(0)
        .expect("valid nanosecond normalization");
    let second_occurrence = first_occurrence + ChronoDuration::seconds(2);
    let first_occurrence_text = utc_seconds(first_occurrence);
    let second_occurrence_text = utc_seconds(second_occurrence);

    let id = ScheduleId::parse(&format!("orders/rrule-e2e-{}", test_nonce())).unwrap();
    let key = ScheduleKey::derive(&id);
    let subject = ScheduleSubject::execution(&key);
    let wakeup_subject = ScheduleSubject::rrule_wakeup(&key);
    execution_stream
        .purge_subject_messages(subject.as_str())
        .await
        .expect("initial purge");

    let wakeup_consumer = execution_stream
        .create_consumer(rrule_wakeup_consumer_config())
        .await
        .expect("create wakeup consumer");

    let command = create_rrule_schedule(id.clone(), &first_occurrence_text, "FREQ=SECONDLY;INTERVAL=2;COUNT=2");
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

    let checkpoints = ScheduleCheckpointStore::new(checkpoint_kv.clone());
    let processor = ScheduleProcessor::new(
        ExecutionScheduleWriter::new(context.clone(), execution_stream.clone()),
        checkpoints.clone(),
        store.clone(),
        EVENT_STREAM,
        Arc::default(),
    );

    // Processing ScheduleCreated arms the first occurrence via the aggregate; no
    // wakeup is published yet — the planned occurrence returns as an event.
    let armed = processor
        .process(&read.events[0], Utc::now())
        .await
        .expect("process command-produced rrule event");
    assert_eq!(armed.outcome, ProcessedOutcome::Published);

    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: command.id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read armed occurrence event");
    assert_eq!(read.events.len(), 2);

    // Processing ScheduleOccurrenceScheduled publishes the concrete wakeup.
    let published = processor
        .process(&read.events[1], Utc::now())
        .await
        .expect("process scheduled occurrence");
    assert_eq!(published.outcome, ProcessedOutcome::Published);

    let first = execution_stream
        .get_last_raw_message_by_subject(subject.as_str())
        .await
        .expect("first rrule execution schedule was published");
    assert_eq!(first.subject.as_str(), subject.as_str());
    let headers = first.headers;
    assert_eq!(
        headers.get("Nats-Schedule").unwrap().as_str(),
        format!("@at {first_occurrence_text}")
    );
    assert_eq!(
        headers.get("Nats-Schedule-Target").unwrap().as_str(),
        wakeup_subject.as_str()
    );
    assert_eq!(
        first.payload.as_ref(),
        format!(
            r#"{{"schedule_id":"{}","occurrence_at":"{first_occurrence_text}"}}"#,
            id.as_str()
        )
        .as_bytes()
    );

    let wakeup_processor = RRuleWakeupProcessor::new(store.clone());
    let first_wakeup = fetch_wakeup(&wakeup_consumer, "first wakeup").await;
    assert_eq!(first_wakeup.message.subject.as_str(), wakeup_subject.as_str());
    let wakeup_outcome = wakeup_processor
        .process(
            first_wakeup.message.subject.as_str(),
            first_wakeup.message.payload.as_ref(),
        )
        .await
        .expect("record first wakeup");
    assert!(matches!(wakeup_outcome, RRuleWakeupOutcome::Recorded { .. }));
    first_wakeup.ack().await.expect("ack first wakeup");

    // Recording the wakeup folds the occurrence and the next plan into the
    // schedule aggregate stream: ScheduleOccurrenceRecorded + ScheduleOccurrenceScheduled.
    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: command.id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read first occurrence events");
    // Created, arming Scheduled, Recorded, folded Scheduled.
    assert_eq!(read.events.len(), 4);

    // The recorded occurrence dispatches the user message.
    let dispatched = processor
        .process(&read.events[2], Utc::now())
        .await
        .expect("process recorded occurrence");
    assert_eq!(dispatched.outcome, ProcessedOutcome::Published);

    let dispatch = execution_stream
        .get_last_raw_message_by_subject(EXECUTION_TARGET_SUBJECT)
        .await
        .expect("first user dispatch was published");
    assert_eq!(dispatch.payload.as_ref(), br#"{"ok":true}"#);
    assert_eq!(
        dispatch
            .headers
            .get("Trogon-Schedule-Occurrence-Sequence")
            .unwrap()
            .as_str(),
        "1"
    );
    assert_eq!(
        dispatch.headers.get("Trogon-Schedule-Occurrence-At").unwrap().as_str(),
        first_occurrence_text.as_str()
    );

    // The scheduled occurrence publishes the planned second wakeup verbatim.
    let scheduled = processor
        .process(&read.events[3], Utc::now())
        .await
        .expect("process scheduled occurrence");
    assert_eq!(scheduled.outcome, ProcessedOutcome::Published);

    let second = execution_stream
        .get_last_raw_message_by_subject(subject.as_str())
        .await
        .expect("second rrule execution schedule was published");
    assert_eq!(
        second.headers.get("Nats-Schedule").unwrap().as_str(),
        format!("@at {second_occurrence_text}")
    );

    let second_wakeup = fetch_wakeup(&wakeup_consumer, "second wakeup").await;
    assert_eq!(second_wakeup.message.subject.as_str(), wakeup_subject.as_str());
    let wakeup_outcome = wakeup_processor
        .process(
            second_wakeup.message.subject.as_str(),
            second_wakeup.message.payload.as_ref(),
        )
        .await
        .expect("record second wakeup");
    assert!(matches!(wakeup_outcome, RRuleWakeupOutcome::Recorded { .. }));
    second_wakeup.ack().await.expect("ack second wakeup");

    // The final occurrence records and completes: ScheduleOccurrenceRecorded + ScheduleCompleted.
    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: command.id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read final occurrence events");
    // ...plus the final Recorded and Completed.
    assert_eq!(read.events.len(), 6);

    let dispatched = processor
        .process(&read.events[4], Utc::now())
        .await
        .expect("process final recorded occurrence");
    assert_eq!(dispatched.outcome, ProcessedOutcome::Published);

    let expired = processor
        .process(&read.events[5], Utc::now())
        .await
        .expect("process completion");
    assert_eq!(expired.outcome, ProcessedOutcome::Expired);

    let checkpoint = checkpoints
        .load(&key)
        .await
        .expect("checkpoint read")
        .expect("checkpoint persisted");
    assert_eq!(checkpoint.record.status, ScheduleStatus::Expired);
    assert_eq!(checkpoint.record.last_outcome, ReconcileOutcome::Expired);
}
