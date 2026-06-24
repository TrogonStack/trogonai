use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, UNIX_EPOCH};

use chrono::{DateTime, TimeZone, Utc};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, Event, EventData, EventDecode, EventDecodeOutcome, ReadFrom,
    ReadStreamRequest, ReadStreamResponse, StreamEvent, StreamWritePrecondition,
};
use trogon_std::time::FixedEpochClock;

use super::*;
use crate::commands::domain::{
    Delivery, MessageContent, Schedule, ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
};
use crate::commands::{CreateSchedule, ScheduleNextOccurrence};
use crate::processor::execution::reconciliation::RRULE_WAKEUP_SUBJECT_PREFIX;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum MemoryStoreError {
    #[error("OCC conflict")]
    Conflict,
}

#[derive(Debug, Clone, Default)]
struct MemoryEventStore {
    streams: Arc<Mutex<HashMap<String, Vec<Event>>>>,
}

impl MemoryEventStore {
    fn events(&self, stream_id: &str) -> Vec<StreamEvent> {
        futures::executor::block_on(self.read_stream(ReadStreamRequest {
            stream_id,
            from: ReadFrom::Beginning,
        }))
        .unwrap()
        .events
    }
}

impl StreamRead<str> for MemoryEventStore {
    type Error = MemoryStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let streams = self.streams.lock().unwrap();
        let events = streams.get(request.stream_id).cloned().unwrap_or_default();
        let current_position = stream_position_for_len(events.len());
        let from = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let stream_events = events
            .into_iter()
            .enumerate()
            .filter_map(|(index, event)| {
                let position = u64::try_from(index + 1).unwrap();
                (position >= from).then(|| StreamEvent {
                    stream_id: request.stream_id.to_string(),
                    event,
                    stream_position: trogon_decider_runtime::StreamPosition::try_new(position).unwrap(),
                    recorded_at: Utc::now(),
                })
            })
            .collect();

        Ok(ReadStreamResponse {
            current_position,
            events: stream_events,
        })
    }
}

impl StreamAppend<str> for MemoryEventStore {
    type Error = MemoryStoreError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut streams = self.streams.lock().unwrap();
        let events = streams.entry(request.stream_id.to_string()).or_default();
        let current_position = stream_position_for_len(events.len());
        let precondition_matches = match request.stream_write_precondition {
            StreamWritePrecondition::Any => true,
            StreamWritePrecondition::StreamExists => current_position.is_some(),
            StreamWritePrecondition::NoStream => current_position.is_none(),
            StreamWritePrecondition::At(position) => current_position == Some(position),
        };
        if !precondition_matches {
            return Err(MemoryStoreError::Conflict);
        }
        events.extend(request.events);
        let stream_position =
            trogon_decider_runtime::StreamPosition::try_new(events.len().try_into().unwrap()).unwrap();

        Ok(AppendStreamResponse { stream_position })
    }
}

fn schedule_id() -> ScheduleId {
    ScheduleId::parse("orders/rrule").unwrap()
}

fn stream_position_for_len(len: usize) -> Option<StreamPosition> {
    if len == 0 {
        None
    } else {
        Some(trogon_decider_runtime::StreamPosition::try_new(len.try_into().unwrap()).unwrap())
    }
}

fn occurrence_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 15, 18, 0, 0).unwrap()
}

fn recorded_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 15, 18, 0, 7).unwrap()
}

fn fixed_clock() -> FixedEpochClock {
    FixedEpochClock(UNIX_EPOCH + StdDuration::from_secs(recorded_at().timestamp().try_into().unwrap()))
}

fn wakeup_payload(id: &ScheduleId) -> Vec<u8> {
    RRuleWakeupPayload::new(id.clone(), occurrence_at()).encode().unwrap()
}

fn wakeup_subject(id: &ScheduleId) -> String {
    ScheduleSubject::rrule_wakeup(&ScheduleKey::derive(id))
        .as_str()
        .to_string()
}

fn create_schedule(id: ScheduleId, status: ScheduleEventStatus) -> CreateSchedule {
    CreateSchedule {
        id,
        status,
        schedule: Schedule::rrule("2026-06-15T18:00:00Z", "FREQ=DAILY;COUNT=2", Some("UTC".to_string())).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::default(),
        },
    }
}

async fn store_with_schedule(status: ScheduleEventStatus) -> (MemoryEventStore, ScheduleId) {
    let store = MemoryEventStore::default();
    let id = schedule_id();
    CommandExecution::new(&store, &create_schedule(id.clone(), status))
        .execute()
        .await
        .unwrap();
    // Enabled schedules are armed by the execution processor reacting to
    // ScheduleCreated; arm here so the wakeup matches the planned occurrence.
    if status == ScheduleEventStatus::Scheduled {
        CommandExecution::new(&store, &ScheduleNextOccurrence::new(id.clone(), occurrence_at()))
            .execute()
            .await
            .unwrap();
    }
    (store, id)
}

#[tokio::test]
async fn wakeup_records_occurrence_into_the_schedule_stream() {
    let (store, id) = store_with_schedule(ScheduleEventStatus::Scheduled).await;
    let processor = RRuleWakeupProcessor::with_clock(store.clone(), fixed_clock());

    let outcome = processor
        .process(&wakeup_subject(&id), &wakeup_payload(&id))
        .await
        .unwrap();

    // Created + the arming Scheduled precede the recording, and recording
    // folds the next Scheduled, so the append lands at position 4.
    assert_eq!(
        outcome,
        RRuleWakeupOutcome::Recorded {
            stream_position: trogon_decider_runtime::StreamPosition::try_new(4).unwrap(),
        }
    );
    let events = store.events(id.as_str());
    assert_eq!(events.len(), 4);
    let decoded = v1_event(&events[2]);
    let Some(trogonai_proto::scheduler::schedules::ScheduleEventCase::ScheduleOccurrenceRecorded(recorded)) =
        decoded.event.as_ref()
    else {
        panic!("expected ScheduleOccurrenceRecorded");
    };
    assert_eq!(recorded.occurrence_sequence, Some(1));
    assert_eq!(
        recorded.occurrence_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&occurrence_at()))
    );
    assert_eq!(
        recorded.recorded_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&recorded_at()))
    );

    let decoded = v1_event(&events[3]);
    let Some(trogonai_proto::scheduler::schedules::ScheduleEventCase::ScheduleOccurrenceScheduled(scheduled)) =
        decoded.event.as_ref()
    else {
        panic!("expected ScheduleOccurrenceScheduled for the next occurrence");
    };
    assert_eq!(scheduled.occurrence_sequence, Some(2));
    assert_eq!(
        scheduled.occurrence_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(
            &Utc.with_ymd_and_hms(2026, 6, 16, 18, 0, 0).unwrap()
        ))
    );
}

#[tokio::test]
async fn wakeup_duplicate_is_acknowledgeable_without_a_second_append() {
    let (store, id) = store_with_schedule(ScheduleEventStatus::Scheduled).await;
    let processor = RRuleWakeupProcessor::new(store.clone());

    processor
        .process(&wakeup_subject(&id), &wakeup_payload(&id))
        .await
        .unwrap();
    let duplicate = processor
        .process(&wakeup_subject(&id), &wakeup_payload(&id))
        .await
        .unwrap();

    assert_eq!(duplicate, RRuleWakeupOutcome::DuplicateStale);
    assert_eq!(store.events(id.as_str()).len(), 4);
}

#[tokio::test]
async fn wakeup_for_paused_schedule_without_pending_occurrence_is_duplicate_stale() {
    let (store, id) = store_with_schedule(ScheduleEventStatus::Paused).await;
    let processor = RRuleWakeupProcessor::new(store.clone());

    let outcome = processor
        .process(&wakeup_subject(&id), &wakeup_payload(&id))
        .await
        .unwrap();

    assert_eq!(outcome, RRuleWakeupOutcome::DuplicateStale);
    assert_eq!(store.events(id.as_str()).len(), 1);
}

#[test]
fn command_rejections_map_to_acknowledgeable_wakeup_outcomes() {
    let id = schedule_id();
    assert_eq!(
        wakeup_outcome_from_rejection(RecordScheduleOccurrenceError::ScheduleNotFound { id: id.clone() }).unwrap(),
        RRuleWakeupOutcome::Obsolete {
            reason: RRuleWakeupObsoleteReason::Missing,
        }
    );
    assert_eq!(
        wakeup_outcome_from_rejection(RecordScheduleOccurrenceError::ScheduleDeleted { id: id.clone() }).unwrap(),
        RRuleWakeupOutcome::Obsolete {
            reason: RRuleWakeupObsoleteReason::Deleted,
        }
    );
    assert_eq!(
        wakeup_outcome_from_rejection(RecordScheduleOccurrenceError::OccurrenceAlreadyRecorded {
            id: id.clone(),
            occurrence_at: occurrence_at(),
            last_recorded_at: occurrence_at(),
        })
        .unwrap(),
        RRuleWakeupOutcome::DuplicateStale
    );
    let rejected = wakeup_outcome_from_rejection(RecordScheduleOccurrenceError::MissingStateValue).unwrap_err();
    assert!(matches!(rejected, RRuleWakeupError::CommandRejected { .. }));
    assert!(!rejected.is_transient());
}

#[test]
fn command_storage_errors_are_classified_for_redelivery() {
    let read =
        command_error::<MemoryStoreError, MemoryStoreError>(CommandError::ReadStream(MemoryStoreError::Conflict));
    assert!(read.is_transient());
    assert!(matches!(read, RRuleWakeupError::CommandTransient { .. }));

    let append = command_error::<MemoryStoreError, MemoryStoreError>(CommandError::Append(MemoryStoreError::Conflict));
    assert!(append.is_transient());
    assert!(matches!(append, RRuleWakeupError::CommandTransient { .. }));

    let permanent =
        command_error::<MemoryStoreError, MemoryStoreError>(CommandError::Evolve(EvolveError::UnsupportedEvent));
    assert!(!permanent.is_transient());
    assert!(matches!(permanent, RRuleWakeupError::CommandPermanent { .. }));
}

#[tokio::test]
async fn memory_store_honors_position_reads_and_preconditions() {
    let (store, id) = store_with_schedule(ScheduleEventStatus::Scheduled).await;
    let processor = RRuleWakeupProcessor::new(store.clone());

    processor
        .process(&wakeup_subject(&id), &wakeup_payload(&id))
        .await
        .unwrap();

    let from_second = store
        .read_stream(ReadStreamRequest {
            stream_id: id.as_str(),
            from: ReadFrom::Position(StreamPosition::try_new(2).unwrap()),
        })
        .await
        .unwrap();
    // Created(1), Scheduled(2), Recorded(3), Scheduled(4): reading from
    // position 2 returns the latter three.
    assert_eq!(from_second.events.len(), 3);

    store
        .append_stream(AppendStreamRequest {
            stream_id: id.as_str(),
            stream_write_precondition: StreamWritePrecondition::Any,
            events: Vec::new(),
        })
        .await
        .unwrap();
    store
        .append_stream(AppendStreamRequest {
            stream_id: id.as_str(),
            stream_write_precondition: StreamWritePrecondition::StreamExists,
            events: Vec::new(),
        })
        .await
        .unwrap();
    let conflict = store
        .append_stream(AppendStreamRequest {
            stream_id: id.as_str(),
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: Vec::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(conflict, MemoryStoreError::Conflict);
}

#[test]
fn wakeup_consumer_filter_matches_rrule_subject_prefix() {
    let config = rrule_wakeup_consumer_config();

    assert_eq!(RRULE_WAKEUP_FILTER, format!("{RRULE_WAKEUP_SUBJECT_PREFIX}.>"));
    assert_eq!(config.filter_subject, RRULE_WAKEUP_FILTER);
    assert_eq!(config.durable_name.as_deref(), Some(RRULE_WAKEUP_CONSUMER));
}

#[tokio::test]
async fn wakeup_subject_must_match_payload_schedule_id() {
    let (store, id) = store_with_schedule(ScheduleEventStatus::Scheduled).await;
    let processor = RRuleWakeupProcessor::new(store);
    let other = ScheduleId::parse("orders/other").unwrap();

    let error = processor
        .process(&wakeup_subject(&other), &wakeup_payload(&id))
        .await
        .unwrap_err();

    assert!(matches!(error, RRuleWakeupError::SubjectMismatch { .. }));
}

#[test]
fn subject_input_display_matches_subject_string() {
    let subject = RRuleWakeupSubjectInput::new("scheduler.schedules.execution.v1.rrule.orders");
    assert_eq!(subject.to_string(), "scheduler.schedules.execution.v1.rrule.orders");
}

#[test]
fn occurrence_not_pending_maps_to_duplicate_stale() {
    let id = schedule_id();
    assert_eq!(
        wakeup_outcome_from_rejection(RecordScheduleOccurrenceError::OccurrenceNotPending {
            id: id.clone(),
            occurrence_at: occurrence_at(),
            pending_occurrence_at: None,
        })
        .unwrap(),
        RRuleWakeupOutcome::DuplicateStale
    );
}

#[tokio::test]
async fn invalid_payload_is_rejected() {
    let store = MemoryEventStore::default();
    let processor = RRuleWakeupProcessor::new(store);
    let id = schedule_id();

    let error = processor
        .process(&wakeup_subject(&id), b"not-json")
        .await
        .unwrap_err();

    assert!(matches!(error, RRuleWakeupError::Payload { .. }));
}

fn v1_event(stream_event: &StreamEvent) -> trogonai_proto::scheduler::schedules::v1::ScheduleEvent {
    match trogonai_proto::scheduler::schedules::v1::ScheduleEvent::decode(EventData::new(
        &stream_event.event.r#type,
        &stream_event.event.content,
    ))
    .unwrap()
    {
        EventDecodeOutcome::Decoded(event) => event,
        EventDecodeOutcome::Skipped => panic!("expected schedule event"),
    }
}
