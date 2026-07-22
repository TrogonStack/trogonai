use std::time::Duration;

use buffa::MessageField;
use chrono::{DateTime, Utc};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use trogon_decider_runtime::{HeaderName, Headers, StreamPosition};
use trogonai_proto::scheduler::schedules::v1;

use super::super::testkit::{
    InMemoryExecution, InMemoryKv, MemoryEventStore, foreign_stream_event, malformed_stream_event, recorded_at,
    schedule_id, stream_event, stream_event_with_headers,
};
use super::*;
use crate::commands::domain::{
    Delivery, MessageContent, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule,
    ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
};
use crate::processor::execution::checkpoints::{ReconcileOutcome, ScheduleCheckpointStore, ScheduleStatus};
use crate::processor::execution::execution_schedules::ExecutionScheduleWriter;
use crate::processor::execution::reconciliation::{ScheduleKey, ScheduleSubject, StreamRoutingId};

type Processor = ScheduleProcessor<InMemoryExecution, InMemoryExecution, InMemoryKv, MemoryEventStore>;

struct Harness {
    processor: Processor,
    kv: InMemoryKv,
    execution: InMemoryExecution,
    event_store: MemoryEventStore,
}

impl Harness {
    fn new() -> Self {
        let kv = InMemoryKv::new();
        let execution = InMemoryExecution::new();
        let event_store = MemoryEventStore::default();
        let writer = ExecutionScheduleWriter::new(execution.clone(), execution.clone());
        let processor = ScheduleProcessor::new(
            writer,
            ScheduleCheckpointStore::new(kv.clone()),
            event_store.clone(),
            "SCHEDULER_SCHEDULE_EVENTS",
            Arc::new(ProcessorMetrics::new()),
        );
        Self {
            processor,
            kv,
            execution,
            event_store,
        }
    }

    fn subject(&self, id: &str) -> ScheduleSubject {
        ScheduleSubject::execution(&key_for_stream(id))
    }

    fn checkpoint_key(&self, id: &str) -> String {
        format!("v1.{}", key_for_stream(id).simple())
    }

    async fn process(&self, event: &v1::ScheduleEvent, id: &str, position: u64) -> Processed {
        self.processor
            .process(&stream_event(event, id, position), recorded_at())
            .await
            .expect("durable outcome")
    }

    async fn process_stream(&self, event: &StreamEvent) -> Processed {
        self.processor
            .process(event, recorded_at())
            .await
            .expect("durable outcome")
    }
}

fn now() -> DateTime<Utc> {
    recorded_at()
}

fn message() -> ScheduleMessage {
    ScheduleMessage {
        content: MessageContent::json(r#"{"ok":true}"#),
        headers: ScheduleHeaders::default(),
    }
}

fn created(id: &str, status: ScheduleEventStatus, schedule: Schedule) -> v1::ScheduleEvent {
    created_with_delivery(id, status, schedule, Delivery::nats_event("agent.run").unwrap())
}

fn created_with_delivery(
    id: &str,
    status: ScheduleEventStatus,
    schedule: Schedule,
    delivery: Delivery,
) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: id.to_string(),
                status: MessageField::some(v1::ScheduleStatus::from(status)),
                schedule: MessageField::some(v1::Schedule::try_from(&ScheduleEventSchedule::from(&schedule)).unwrap()),
                delivery: MessageField::some(v1::Delivery::try_from(&ScheduleEventDelivery::from(&delivery)).unwrap()),
                message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&message()))),
            }
            .into(),
        ),
    }
}

fn paused(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::SchedulePaused {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

fn resumed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleResumed {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

fn removed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleRemoved {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

fn occurrence_recorded(id: &str, occurrence_sequence: u64, occurrence_at: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceRecorded {
                schedule_id: id.to_string(),
                occurrence_sequence: Some(occurrence_sequence),
                occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                    &DateTime::parse_from_rfc3339(occurrence_at).unwrap().with_timezone(&Utc),
                )),
                recorded_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&recorded_at())),
            }
            .into(),
        ),
    }
}

fn occurrence_scheduled(id: &str, occurrence_at: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceScheduled {
                schedule_id: id.to_string(),
                occurrence_sequence: Some(2),
                occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                    &DateTime::parse_from_rfc3339(occurrence_at).unwrap().with_timezone(&Utc),
                )),
                scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&recorded_at())),
            }
            .into(),
        ),
    }
}

fn completed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCompleted {
                schedule_id: id.to_string(),
                last_occurrence_sequence: Some(2),
            }
            .into(),
        ),
    }
}

fn every() -> Schedule {
    Schedule::every(Duration::from_secs(30)).unwrap()
}

fn checkpoint_record(id: &str, schedule: Schedule, delivery: Delivery) -> ScheduleCheckpointRecord {
    ScheduleCheckpointRecord {
        schedule_id: schedule_id(id),
        status: ScheduleStatus::Scheduled,
        schedule,
        delivery,
        message: message(),
        last_applied_stream_position: StreamPosition::try_new(1).unwrap(),
        last_applied_event_id: Some("event-1".to_string()),
        last_outcome: ReconcileOutcome::Published,
    }
}

fn key_for_stream(id: &str) -> ScheduleKey {
    ScheduleKey::for_stream(&StreamRoutingId::from(id))
}

#[test]
fn unsupported_outcome_has_a_metric_label() {
    assert_eq!(ProcessedOutcome::Unsupported.metric_label(), "unsupported");
}

#[tokio::test]
async fn enabled_create_publishes_and_persists() {
    let harness = Harness::new();
    let id = "orders/created";

    let processed = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::Published);
    assert_eq!(processed.ack, AckAction::Ack);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    assert!(harness.kv.contains(&harness.checkpoint_key(id)));
}

#[tokio::test]
async fn enabled_create_matches_the_e2e_io_contract_with_mocks() {
    let harness = Harness::new();
    let id = "orders/created";
    let key = key_for_stream(id);
    let subject = harness.subject(id);
    let event = created(id, ScheduleEventStatus::Scheduled, every());
    let stream_event = stream_event(&event, id, 1);
    let event_id = stream_event.event.id.to_string();

    let processed = harness.processor.process(&stream_event, now()).await.unwrap();

    assert_eq!(processed.outcome, ProcessedOutcome::Published);
    assert_eq!(processed.ack, AckAction::Ack);
    assert_eq!(harness.execution.scheduled_count(subject.as_str()), 1);

    let headers = harness.execution.headers_for(subject.as_str()).unwrap();
    assert_eq!(headers.get("Nats-Schedule").unwrap().as_str(), "@every 30s");
    assert_eq!(headers.get("Nats-Schedule-Target").unwrap().as_str(), "agent.run");
    assert_eq!(headers.get("Nats-Msg-Id").unwrap().as_str(), event_id.as_str());
    assert_eq!(headers.get("Content-Type").unwrap().as_str(), "application/json");
    assert_eq!(headers.get("Trogon-Schedule-Key").unwrap().as_str(), key.simple());
    assert_eq!(
        headers.get("Trogon-Schedule-Id-B64").unwrap().as_str(),
        "b3JkZXJzL2NyZWF0ZWQ"
    );
    assert_eq!(
        harness.execution.payload_for(subject.as_str()).unwrap().as_ref(),
        br#"{"ok":true}"#
    );

    let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
        .load(&key)
        .await
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(checkpoint.schedule_id.as_str(), id);
    assert_eq!(checkpoint.key(), key);
    assert_eq!(checkpoint.subject(), subject);
    assert_eq!(checkpoint.status, ScheduleStatus::Scheduled);
    assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 1);
    assert_eq!(checkpoint.last_applied_event_id.as_deref(), Some(event_id.as_str()));
    assert_eq!(checkpoint.last_outcome, ReconcileOutcome::Published);
    assert_eq!(checkpoint.message.content.as_slice(), br#"{"ok":true}"#);
    assert_eq!(checkpoint.message.content.content_type().as_str(), "application/json");
}

#[tokio::test]
async fn trace_context_is_present_on_the_execution_publish() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let harness = Harness::new();
    let id = "orders/created";
    let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let headers = Headers::one(HeaderName::new("traceparent").unwrap(), traceparent).unwrap();
    let event = created(id, ScheduleEventStatus::Scheduled, every());

    self_process_with_headers(&harness, &event, id, 1, headers).await;

    let published = harness.execution.headers_for(harness.subject(id).as_str()).unwrap();
    assert!(
        published
            .get("traceparent")
            .unwrap()
            .as_str()
            .contains("4bf92f3577b34da6a3ce929d0e0e4736"),
        "execution publish carries the originating trace"
    );
}

async fn self_process_with_headers(
    harness: &Harness,
    event: &v1::ScheduleEvent,
    id: &str,
    position: u64,
    headers: Headers,
) -> Processed {
    harness
        .processor
        .process(&stream_event_with_headers(event, id, position, headers), now())
        .await
        .expect("durable outcome")
}

#[tokio::test]
async fn past_due_at_expires_without_publishing() {
    let harness = Harness::new();
    let id = "orders/once";
    let past = Schedule::At {
        at: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    };

    let processed = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, past), id, 1)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::Expired);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
}

#[tokio::test]
async fn resuming_a_past_due_at_reports_a_distinct_resumed_expired_outcome() {
    let harness = Harness::new();
    let id = "orders/once";
    let past = Schedule::At {
        at: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    };

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, past), id, 1)
        .await;
    harness.process(&paused(id), id, 2).await;
    let processed = harness.process(&resumed(id), id, 3).await;

    assert_eq!(processed.outcome, ProcessedOutcome::ResumedExpired);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
}

#[tokio::test]
async fn enabled_rrule_arms_then_publishes_the_next_one_shot_wakeup() {
    let harness = Harness::new();
    let id = "recurring";
    let wakeup = ScheduleSubject::rrule_wakeup(&key_for_stream(id));
    let create = crate::CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: message(),
    };
    CommandExecution::new(&harness.event_store, &create)
        .execute()
        .await
        .expect("seed schedule");

    // Created arms the occurrence; the Scheduled event publishes the wakeup.
    let events = harness.event_store.events(id);
    harness.process_stream(&events[0]).await;
    let events = harness.event_store.events(id);
    let published = harness.process_stream(&events[1]).await;

    assert_eq!(published.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    let headers = harness.execution.headers_for(harness.subject(id).as_str()).unwrap();
    assert_eq!(
        headers.get("Nats-Schedule").unwrap().as_str(),
        "@at 2026-06-04T00:00:00Z"
    );
    assert_eq!(headers.get("Nats-Schedule-Target").unwrap().as_str(), wakeup.as_str());
    assert_eq!(
        harness
            .execution
            .payload_for(harness.subject(id).as_str())
            .unwrap()
            .as_ref(),
        br#"{"schedule_id":"recurring","occurrence_at":"2026-06-04T00:00:00Z"}"#
    );
}

#[tokio::test]
async fn rrule_recorded_dispatches_scheduled_publishes_and_completed_expires() {
    let harness = Harness::new();
    let id = "recurring";

    // Seed the schedule into the event store so the arm command can read it.
    let create = crate::CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: message(),
    };
    CommandExecution::new(&harness.event_store, &create)
        .execute()
        .await
        .expect("seed schedule");

    // Processing ScheduleCreated arms the next occurrence via the aggregate
    // (no recurrence math in the processor) and publishes no wakeup itself.
    let created_events = harness.event_store.events(id);
    let armed = harness.process_stream(&created_events[0]).await;
    assert_eq!(armed.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

    // The aggregate emitted ScheduleOccurrenceScheduled (06-03 is before the
    // grace floor of now=06-04, so the first armed occurrence is 06-04).
    let armed_events = harness.event_store.events(id);
    assert_eq!(armed_events.len(), 2);
    let published = harness.process_stream(&armed_events[1]).await;
    assert_eq!(published.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    assert_eq!(
        harness
            .execution
            .headers_for(harness.subject(id).as_str())
            .unwrap()
            .get("Nats-Schedule")
            .unwrap()
            .as_str(),
        "@at 2026-06-04T00:00:00Z"
    );

    // A recorded occurrence only dispatches the user message; it never
    // computes or publishes the next wakeup.
    let dispatched = harness
        .process(&occurrence_recorded(id, 1, "2026-06-04T00:00:00Z"), id, 3)
        .await;
    assert_eq!(dispatched.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count("agent.run"), 1);
    assert_eq!(
        harness.execution.payload_for("agent.run").unwrap().as_ref(),
        br#"{"ok":true}"#
    );
    let dispatch_headers = harness.execution.headers_for("agent.run").unwrap();
    assert_eq!(
        dispatch_headers
            .get("Trogon-Schedule-Occurrence-Sequence")
            .unwrap()
            .as_str(),
        "1"
    );
    assert_eq!(
        dispatch_headers.get("Trogon-Schedule-Occurrence-At").unwrap().as_str(),
        "2026-06-04T00:00:00Z"
    );
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    // A scheduled occurrence publishes exactly the planned wakeup.
    let scheduled = harness
        .process(&occurrence_scheduled(id, "2026-06-05T00:00:00Z"), id, 4)
        .await;
    assert_eq!(scheduled.outcome, ProcessedOutcome::Published);
    assert_eq!(
        harness
            .execution
            .headers_for(harness.subject(id).as_str())
            .unwrap()
            .get("Nats-Schedule")
            .unwrap()
            .as_str(),
        "@at 2026-06-05T00:00:00Z"
    );

    // Completion purges the execution subject and expires the checkpoint.
    let expired = harness.process(&completed(id), id, 5).await;
    assert_eq!(expired.outcome, ProcessedOutcome::Expired);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
        .load(&key_for_stream(id))
        .await
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(checkpoint.status, ScheduleStatus::Expired);
}

#[tokio::test]
async fn arm_is_idempotent_when_already_armed() {
    let harness = Harness::new();
    let id = "recurring";
    let create = crate::CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: message(),
    };
    CommandExecution::new(&harness.event_store, &create)
        .execute()
        .await
        .expect("seed schedule");
    CommandExecution::new(
        &harness.event_store,
        &crate::ScheduleNextOccurrence::new(schedule_id(id), recorded_at()),
    )
    .execute()
    .await
    .expect("arm once");
    let armed_len = harness.event_store.events(id).len();
    assert_eq!(armed_len, 2);

    // Reprocessing ScheduleCreated re-issues the arm command, but the schedule
    // is already armed, so it must be an idempotent no-op (no second Scheduled).
    let created = harness.event_store.events(id);
    let processed = harness.process_stream(&created[0]).await;
    assert_eq!(processed.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.event_store.events(id).len(), armed_len);
}

#[tokio::test]
async fn arm_is_idempotent_when_completion_was_appended_before_checkpoint_save() {
    let harness = Harness::new();
    let id = "recurring";
    let create = crate::CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=1", None).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: message(),
    };
    CommandExecution::new(&harness.event_store, &create)
        .execute()
        .await
        .expect("seed schedule");

    let created = harness.event_store.events(id);
    harness.kv.fail_next_create();
    let retry = harness.processor.process(&created[0], recorded_at()).await.unwrap_err();
    assert!(matches!(retry, RetryableError::Checkpoint { .. }));
    assert_eq!(harness.event_store.events(id).len(), 2);

    let processed = harness.process_stream(&created[0]).await;
    assert_eq!(processed.outcome, ProcessedOutcome::Published);

    let completed = harness.event_store.events(id);
    let expired = harness.process_stream(&completed[1]).await;
    assert_eq!(expired.outcome, ProcessedOutcome::Expired);
}

#[tokio::test]
async fn arm_reports_missing_or_deleted_schedules_for_retry() {
    let harness = Harness::new();
    let trace_headers = HeaderMap::new();

    let missing = harness
        .processor
        .apply_action(
            &ReconcileAction::ArmNext {
                schedule_id: schedule_id("missing"),
                now: recorded_at(),
            },
            "event-missing",
            &trace_headers,
        )
        .await
        .unwrap_err();
    assert!(matches!(missing, RetryableError::ArmSchedule { .. }));

    CommandExecution::new(
        &harness.event_store,
        &crate::CreateSchedule {
            id: schedule_id("deleted"),
            status: ScheduleEventStatus::Scheduled,
            schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: message(),
        },
    )
    .execute()
    .await
    .expect("seed schedule");
    CommandExecution::new(
        &harness.event_store,
        &crate::RemoveSchedule::new(schedule_id("deleted")),
    )
    .execute()
    .await
    .expect("delete schedule");

    let deleted = harness
        .processor
        .apply_action(
            &ReconcileAction::ArmNext {
                schedule_id: schedule_id("deleted"),
                now: recorded_at(),
            },
            "event-deleted",
            &trace_headers,
        )
        .await
        .unwrap_err();
    assert!(matches!(deleted, RetryableError::ArmSchedule { .. }));
}

#[tokio::test]
async fn rrule_pause_purges_then_resume_rearms() {
    let harness = Harness::new();
    let id = "recurring";
    let create = crate::CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: message(),
    };
    CommandExecution::new(&harness.event_store, &create)
        .execute()
        .await
        .expect("seed schedule");

    // Created -> arm -> publish the first wakeup.
    let events = harness.event_store.events(id);
    harness.process_stream(&events[0]).await;
    let events = harness.event_store.events(id);
    harness.process_stream(&events[1]).await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    // Pause purges the live timer.
    CommandExecution::new(&harness.event_store, &crate::PauseSchedule::new(schedule_id(id)))
        .execute()
        .await
        .expect("pause");
    let events = harness.event_store.events(id);
    let paused = harness.process_stream(events.last().unwrap()).await;
    assert_eq!(paused.outcome, ProcessedOutcome::Purged);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

    // Resume re-arms a fresh occurrence and re-publishes the wakeup.
    CommandExecution::new(&harness.event_store, &crate::ResumeSchedule::new(schedule_id(id)))
        .execute()
        .await
        .expect("resume");
    let events = harness.event_store.events(id);
    harness.process_stream(events.last().unwrap()).await; // Resumed -> ArmNext -> Scheduled
    let events = harness.event_store.events(id);
    harness.process_stream(events.last().unwrap()).await; // Scheduled -> publish wakeup
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[tokio::test]
async fn paused_rrule_recorded_occurrence_still_dispatches_user_message() {
    let harness = Harness::new();
    let id = "recurring";
    let create = crate::CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: message(),
    };
    CommandExecution::new(&harness.event_store, &create)
        .execute()
        .await
        .expect("seed schedule");

    let events = harness.event_store.events(id);
    harness.process_stream(&events[0]).await;
    let events = harness.event_store.events(id);
    harness.process_stream(&events[1]).await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    CommandExecution::new(&harness.event_store, &crate::PauseSchedule::new(schedule_id(id)))
        .execute()
        .await
        .expect("pause");
    let events = harness.event_store.events(id);
    let paused = harness.process_stream(events.last().unwrap()).await;
    assert_eq!(paused.outcome, ProcessedOutcome::Purged);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

    let occurrence_at = DateTime::parse_from_rfc3339("2026-06-04T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    CommandExecution::new(
        &harness.event_store,
        &crate::RecordScheduleOccurrence::new(schedule_id(id), occurrence_at, recorded_at()),
    )
    .execute()
    .await
    .expect("record paused occurrence");

    let events = harness.event_store.events(id);
    let dispatched = harness.process_stream(events.last().unwrap()).await;
    assert_eq!(dispatched.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count("agent.run"), 1);
    assert_eq!(
        harness.execution.payload_for("agent.run").unwrap().as_ref(),
        br#"{"ok":true}"#
    );
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

    let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
        .load(&key_for_stream(id))
        .await
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(checkpoint.status, ScheduleStatus::Paused);
}

#[tokio::test]
async fn direct_dispatch_action_uses_the_event_id_without_disambiguation() {
    let harness = Harness::new();
    let request = crate::processor::execution::reconciliation::DispatchRequest::build(
        &schedule_id("recurring"),
        &Delivery::nats_event("agent.run").unwrap(),
        &message(),
    )
    .unwrap();
    let trace_headers = HeaderMap::new();

    harness
        .processor
        .apply_action(&ReconcileAction::Dispatch(request), "event-2", &trace_headers)
        .await
        .unwrap();

    assert_eq!(harness.execution.scheduled_count("agent.run"), 1);
    let headers = harness.execution.headers_for("agent.run").unwrap();
    assert_eq!(headers.get("Nats-Msg-Id").unwrap().as_str(), "event-2");
    assert_eq!(
        harness.execution.payload_for("agent.run").unwrap().as_ref(),
        br#"{"ok":true}"#
    );
}

#[tokio::test]
async fn rrule_occurrence_event_reports_missing_checkpoints() {
    let harness = Harness::new();

    let error = harness
        .processor
        .process(
            &stream_event(&occurrence_recorded("missing", 1, "2026-06-04T00:00:00Z"), "missing", 2),
            now(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, RetryableError::MissingCheckpoint { .. }));
}

#[tokio::test]
async fn rrule_occurrence_event_acks_non_rrule_checkpoints_as_duplicate_stale() {
    let harness = Harness::new();
    let id = "recurring";
    let record = checkpoint_record(id, every(), Delivery::nats_event("agent.run").unwrap());
    ScheduleCheckpointStore::new(harness.kv.clone())
        .save(&record, None)
        .await
        .unwrap();

    let processed = harness
        .process(&occurrence_recorded(id, 1, "2026-06-04T00:00:00Z"), id, 2)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::DuplicateStale);
}

#[tokio::test]
async fn rrule_occurrence_event_rejects_scheduler_internal_targets() {
    let harness = Harness::new();
    let id = "recurring";
    let rrule = Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap();
    let record = checkpoint_record(id, rrule, Delivery::nats_event(harness.subject(id).as_str()).unwrap());
    ScheduleCheckpointStore::new(harness.kv.clone())
        .save(&record, None)
        .await
        .unwrap();

    let processed = harness
        .process(&occurrence_recorded(id, 1, "2026-06-03T00:00:00Z"), id, 2)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
}

#[tokio::test]
async fn paused_create_stores_without_publishing() {
    let harness = Harness::new();
    let id = "orders/created";

    let processed = harness
        .process(&created(id, ScheduleEventStatus::Paused, every()), id, 1)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::StoredPaused);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    assert!(harness.kv.contains(&harness.checkpoint_key(id)));
}

#[tokio::test]
async fn pause_then_resume_purges_then_republishes() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    let paused = harness.process(&paused(id), id, 2).await;
    assert_eq!(paused.outcome, ProcessedOutcome::Purged);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

    let resumed = harness.process(&resumed(id), id, 3).await;
    assert_eq!(resumed.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[tokio::test]
async fn remove_purges_and_marks_removed() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    let removed = harness.process(&removed(id), id, 2).await;

    assert_eq!(removed.outcome, ProcessedOutcome::Purged);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
}

#[tokio::test]
async fn duplicate_redelivery_after_ack_is_a_no_op() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    // Redeliver the exact same record (same stream position).
    let again = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(again.outcome, ProcessedOutcome::DuplicateStale);
    // No second schedule was published.
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[tokio::test]
async fn stale_record_below_watermark_is_a_no_op() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 5)
        .await;
    // A record below the stored watermark (position 3 < 5).
    let stale = harness.process(&paused(id), id, 3).await;
    assert_eq!(stale.outcome, ProcessedOutcome::DuplicateStale);
    // The schedule was not purged by the stale pause.
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[tokio::test]
async fn crash_after_execution_schedule_write_before_checkpoint_write_retries_and_converges() {
    let harness = Harness::new();
    let id = "orders/created";
    // Execution schedule write (purge+publish) succeeds, but the checkpoint create fails transiently.
    harness.kv.fail_next_create();

    let error = harness
        .processor
        .process(
            &stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1),
            now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, RetryableError::Checkpoint { .. }));
    assert!(!harness.kv.contains(&harness.checkpoint_key(id)));

    // Redelivery: purge-then-publish converges to exactly one schedule and
    // the checkpoint write now succeeds.
    let retried = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(retried.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    assert!(harness.kv.contains(&harness.checkpoint_key(id)));
}

#[tokio::test]
async fn transient_execution_schedule_failure_does_not_advance_checkpoint() {
    let harness = Harness::new();
    let id = "orders/created";
    harness.execution.fail_next_publish();

    let error = harness
        .processor
        .process(
            &stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1),
            now(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, RetryableError::ExecutionSchedule { .. }));
    assert!(!harness.kv.contains(&harness.checkpoint_key(id)));
}

#[tokio::test]
async fn malformed_record_is_durably_recorded_before_term() {
    let harness = Harness::new();

    let processed = harness
        .processor
        .process(&malformed_stream_event(9), now())
        .await
        .expect("durable outcome");

    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
    assert_eq!(processed.ack, AckAction::Term);
    assert!(harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.9"));
}

#[tokio::test]
async fn foreign_event_is_skipped_and_acked() {
    let harness = Harness::new();

    let processed = harness
        .processor
        .process(&foreign_stream_event(4), now())
        .await
        .expect("durable outcome");

    assert_eq!(processed.outcome, ProcessedOutcome::SkippedForeign);
    assert_eq!(processed.ack, AckAction::Ack);
}

#[tokio::test]
async fn mismatched_stream_routing_is_durably_recorded_before_term() {
    let harness = Harness::new();
    let id = "orders/created";
    let mut event = stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1);
    event.stream_id = "wrong-stream".to_string();

    let processed = harness.processor.process(&event, now()).await.expect("durable outcome");

    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
    assert_eq!(processed.ack, AckAction::Term);
    assert!(harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.1"));
}

#[tokio::test]
async fn schedule_change_without_checkpoint_is_a_transient_retry_not_a_poison() {
    let harness = Harness::new();
    let id = "orders/created";

    // A pause arrives before any checkpoint exists (e.g. KV loss before replay).
    let error = harness
        .processor
        .process(&stream_event(&paused(id), id, 2), now())
        .await
        .unwrap_err();

    assert!(matches!(error, RetryableError::MissingCheckpoint { .. }));
    assert_eq!(
        error.to_string(),
        "no checkpoint yet for schedule 'orders/created', retrying"
    );
    // It is not written as a poison record.
    assert!(!harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.2"));
    let _ = schedule_id(id);
}

#[tokio::test]
async fn stale_corrupt_checkpoint_create_redelivery_repairs_the_record() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    harness.kv.corrupt_definition(&harness.checkpoint_key(id));

    let again = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(again.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
        .load(&key_for_stream(id))
        .await
        .expect("repaired checkpoint decodes")
        .unwrap()
        .record;
    assert_eq!(checkpoint.status, ScheduleStatus::Scheduled);
    assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 1);
}

#[tokio::test]
async fn truncated_corrupt_checkpoint_redelivery_repairs_then_resume_succeeds() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    // Truncate the blob inside the trailing nested snapshot; the envelope
    // fields parsed before the truncation point identify the duplicate.
    harness.kv.truncate_tail(&harness.checkpoint_key(id), 5);

    let again = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(again.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    // The rewritten record is decodable again, so a later pause/resume
    // cycle reconciles instead of hitting the corrupt path.
    let paused = harness.process(&paused(id), id, 2).await;
    assert_eq!(paused.outcome, ProcessedOutcome::Purged);
    let resumed = harness.process(&resumed(id), id, 3).await;
    assert_eq!(resumed.outcome, ProcessedOutcome::Published);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[tokio::test]
async fn corrupt_resumed_duplicate_acks_without_repair() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    harness.process(&paused(id), id, 2).await;
    harness.process(&resumed(id), id, 3).await;
    harness.kv.truncate_tail(&harness.checkpoint_key(id), 5);

    let again = harness.process(&resumed(id), id, 3).await;

    assert_eq!(again.outcome, ProcessedOutcome::DuplicateStale);
    assert_eq!(again.ack, AckAction::Ack);
}

#[tokio::test]
async fn inflated_corrupt_watermark_does_not_skip_newer_events() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    harness.kv.corrupt_definition(&harness.checkpoint_key(id));
    harness.kv.inflate_watermark(&harness.checkpoint_key(id), 99);

    let processed = harness.process(&paused(id), id, 2).await;
    assert_eq!(processed.outcome, ProcessedOutcome::Purged);
    assert_eq!(processed.ack, AckAction::Ack);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
}

#[tokio::test]
async fn corrupt_cached_checkpoint_control_event_purges_and_overwrites_tombstone() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    // Corrupt the rebuildable cache entry so it can no longer be decoded.
    harness.kv.corrupt(&harness.checkpoint_key(id));

    let processed = harness.process(&paused(id), id, 2).await;
    assert_eq!(processed.outcome, ProcessedOutcome::Purged);
    assert_eq!(processed.ack, AckAction::Ack);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

    let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
        .load(&key_for_stream(id))
        .await
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(checkpoint.status, ScheduleStatus::Paused);
    assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 2);

    let processed = harness
        .processor
        .process(&stream_event(&resumed(id), id, 3), now())
        .await
        .expect("durable outcome");
    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
    assert_eq!(processed.ack, AckAction::Term);
}

#[tokio::test]
async fn corrupt_cached_checkpoint_create_event_rebuilds_checkpoint() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    harness.kv.corrupt(&harness.checkpoint_key(id));

    let processed = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 2)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::Published);
    assert_eq!(processed.ack, AckAction::Ack);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
        .load(&key_for_stream(id))
        .await
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(checkpoint.status, ScheduleStatus::Scheduled);
    assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 2);
}

#[tokio::test]
async fn transient_load_failure_is_retried() {
    let harness = Harness::new();
    let id = "orders/created";
    harness.kv.fail_next_entry();

    let error = harness
        .processor
        .process(
            &stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1),
            now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, RetryableError::Checkpoint { .. }));
    assert_eq!(
        error.to_string(),
        "transient checkpoint failure: scheduler checkpoint backend failed: failed getting entry"
    );
}

#[tokio::test]
async fn transient_purge_failure_is_retried() {
    let harness = Harness::new();
    let id = "orders/created";
    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;

    harness.execution.fail_next_purge();
    let error = harness
        .processor
        .process(&stream_event(&paused(id), id, 2), now())
        .await
        .unwrap_err();
    assert!(matches!(error, RetryableError::ExecutionSchedule { .. }));
    assert_eq!(
        error.to_string(),
        "transient execution schedule failure: execution subject purge failed: simulated purge failure"
    );
}

#[tokio::test]
async fn checkpoint_create_race_loser_acks_duplicate_stale() {
    let harness = Harness::new();
    let id = "orders/created";

    // The winner already applied position 1 and saved its checkpoint.
    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

    // The loser loaded before the winner's first write landed, so it sees
    // no checkpoint and its save is a create that hits AlreadyExists.
    harness.kv.miss_next_entry();
    let processed = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::DuplicateStale);
    assert_eq!(processed.ack, AckAction::Ack);
    // The loser's idempotent upsert did not duplicate the schedule.
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[tokio::test]
async fn checkpoint_update_cas_loss_retries_and_converges() {
    let harness = Harness::new();
    let id = "orders/created";
    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;

    // The update loses a CAS race; the reloaded checkpoint does not cover
    // this position yet, so the record retries instead of poisoning.
    harness.kv.conflict_next_update();
    let error = harness
        .processor
        .process(&stream_event(&paused(id), id, 2), now())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RetryableError::Checkpoint {
            source: CheckpointStoreError::Conflict
        }
    ));
    assert!(!harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.2"));

    // Redelivery reconciles against the fresh checkpoint and converges.
    let retried = harness.process(&paused(id), id, 2).await;
    assert_eq!(retried.outcome, ProcessedOutcome::Purged);
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
}

#[tokio::test]
async fn transient_checkpoint_update_failure_is_retried() {
    let harness = Harness::new();
    let id = "orders/created";
    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;

    // The remove purges fine, but the checkpoint update write fails transiently.
    harness.kv.fail_next_update();
    let error = harness
        .processor
        .process(&stream_event(&removed(id), id, 2), now())
        .await
        .unwrap_err();
    assert!(matches!(error, RetryableError::Checkpoint { .. }));
}

#[tokio::test]
async fn kv_loss_is_recovered_by_replaying_creation() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert!(harness.kv.contains(&harness.checkpoint_key(id)));

    // Lose the rebuildable checkpoint cache.
    harness.kv.clear();
    assert!(!harness.kv.contains(&harness.checkpoint_key(id)));

    // A durable reset replays creation (DeliverPolicy::All); the checkpoint is
    // rebuilt and the execution schedule converges to exactly one message.
    let replayed = harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    assert_eq!(replayed.outcome, ProcessedOutcome::Published);
    assert!(harness.kv.contains(&harness.checkpoint_key(id)));
    assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
}

#[test]
fn record_redelivery_is_infallible() {
    Harness::new().processor.record_redelivery();
}

#[tokio::test]
async fn corrupt_cached_checkpoint_resume_event_is_poisoned() {
    let harness = Harness::new();
    let id = "orders/created";

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    harness.kv.corrupt(&harness.checkpoint_key(id));

    let processed = harness.process(&resumed(id), id, 2).await;

    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
    assert_eq!(processed.ack, AckAction::Term);
}

#[test]
fn outcome_from_maps_duplicate_stale() {
    assert_eq!(
        outcome_from(ReconcileOutcome::DuplicateStale),
        ProcessedOutcome::DuplicateStale
    );
    assert_eq!(
        outcome_from(ReconcileOutcome::Unsupported),
        ProcessedOutcome::Unsupported
    );
    assert_eq!(
        outcome_from(ReconcileOutcome::Unknown),
        ProcessedOutcome::DuplicateStale
    );
}

#[test]
fn recover_corrupt_checkpoint_without_revision_is_a_no_op() {
    let change = ScheduleChange::Paused {
        schedule_id: schedule_id("orders/created"),
    };
    assert!(
        recover_corrupt_checkpoint(&change, StreamPosition::try_new(2).unwrap(), Some("evt"), None, now())
            .unwrap()
            .is_none()
    );
}

#[test]
fn recover_corrupt_checkpoint_removed_event_purges() {
    let id = schedule_id("orders/created");
    let change = ScheduleChange::Removed {
        schedule_id: id.clone(),
    };
    let recovery = recover_corrupt_checkpoint(
        &change,
        StreamPosition::try_new(2).unwrap(),
        Some("evt"),
        Some(1),
        now(),
    )
    .unwrap()
    .expect("removed corrupt recovery should purge");

    assert!(matches!(recovery.action, ReconcileAction::Purge(_)));
    assert_eq!(recovery.next_checkpoint.status, ScheduleStatus::Removed);
}

#[tokio::test]
async fn schedule_request_failure_during_create_is_poisoned() {
    let harness = Harness::new();
    let id = "orders/created";
    let subject = ScheduleSubject::execution(&key_for_stream(id));
    let delivery = Delivery::nats_event(subject.as_str()).unwrap();

    let processed = harness
        .process(
            &created_with_delivery(id, ScheduleEventStatus::Scheduled, every(), delivery),
            id,
            1,
        )
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
    assert_eq!(processed.ack, AckAction::Term);
}

#[tokio::test]
async fn corrupt_create_with_invalid_delivery_is_poisoned_on_schedule_request() {
    let harness = Harness::new();
    let id = "orders/created";
    let subject = ScheduleSubject::execution(&key_for_stream(id));
    let delivery = Delivery::nats_event(subject.as_str()).unwrap();

    harness
        .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
        .await;
    harness.kv.corrupt(&harness.checkpoint_key(id));

    let processed = harness
        .process(
            &created_with_delivery(id, ScheduleEventStatus::Scheduled, every(), delivery),
            id,
            2,
        )
        .await;

    assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
    assert_eq!(processed.ack, AckAction::Term);
}
