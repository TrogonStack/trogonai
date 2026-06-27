use trogon_decider_runtime::{CommandError, CommandExecution, ImmediateSnapshotTaskScheduler};

use super::*;
use crate::commands::domain as command_domain;
use crate::{
    CreateSchedule, GetSchedule, ListSchedules, MessageContent, MessageEnvelope, MessageHeaders, PauseSchedule,
    RemoveSchedule, ResumeSchedule, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
    ScheduleId, ScheduleWriteCondition,
};

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}
fn command_schedule_id(id: &str) -> command_domain::ScheduleId {
    command_domain::ScheduleId::parse(id).unwrap()
}

fn base_schedule(id: &str) -> Schedule {
    Schedule {
        id: id.to_string(),
        status: ScheduleEventStatus::Scheduled,
        completed: false,
        next_occurrence_at: None,
        last_occurrence_at: None,
        schedule: ScheduleEventSchedule::Every {
            every: std::time::Duration::from_secs(30),
        },
        delivery: ScheduleEventDelivery::NatsMessage {
            subject: "agent.run".to_string(),
            ttl: None,
            source: None,
        },
        message: MessageEnvelope {
            // Mirrors the content type the command domain emits by default
            // (`MessageContent::from_static`), proving content type round-trips.
            content: MessageContent::from_static("application/octet-stream", r#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        },
    }
}

fn command_base_schedule(id: &str) -> CreateSchedule {
    CreateSchedule {
        id: command_schedule_id(id),
        status: command_domain::ScheduleEventStatus::Scheduled,
        schedule: command_domain::Schedule::every(std::time::Duration::from_secs(30)).unwrap(),
        delivery: command_domain::Delivery::nats_event("agent.run").unwrap(),
        message: command_domain::ScheduleMessage {
            content: command_domain::MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: command_domain::ScheduleHeaders::default(),
        },
    }
}

fn expected_schedule(id: &str) -> Schedule {
    base_schedule(id)
}

#[tokio::test]
async fn mock_scheduler_store_covers_crud_and_read_model() {
    let store = MockSchedulerStore::new();
    store.seed_schedule(base_schedule("seeded"));

    let seeded = store
        .get_schedule(GetSchedule::new(ScheduleId::parse("seeded").unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seeded, expected_schedule("seeded"));

    CommandExecution::new(&store, &command_base_schedule("alpha"))
        .execute()
        .await
        .unwrap();
    let alpha = store
        .get_schedule(GetSchedule::new(ScheduleId::parse("alpha").unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alpha, expected_schedule("alpha"));

    CommandExecution::new(&store, &PauseSchedule::new(command_schedule_id("alpha")))
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
    assert_eq!(
        store
            .get_schedule(GetSchedule::new(ScheduleId::parse("alpha").unwrap()))
            .await
            .unwrap()
            .unwrap()
            .status,
        ScheduleEventStatus::Paused
    );

    let listed = store.list_schedules(ListSchedules).await.unwrap();
    assert_eq!(listed.len(), 2);

    CommandExecution::new(&store, &RemoveSchedule::new(command_schedule_id("alpha")))
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
    assert!(
        store
            .get_schedule(GetSchedule::new(ScheduleId::parse("alpha").unwrap()))
            .await
            .unwrap()
            .is_none()
    );

    let deleted_error = CommandExecution::new(&store, &command_base_schedule("alpha"))
        .execute()
        .await
        .unwrap_err();
    assert!(matches!(
        deleted_error,
        CommandError::Append(SchedulerError::OptimisticConcurrencyConflict {
            expected: StreamWritePrecondition::NoStream,
            current_position: Some(_),
            ..
        })
    ));
}

#[tokio::test]
async fn mock_scheduler_store_rejects_invalid_specs_and_state_errors() {
    let store = MockSchedulerStore::new();
    let invalid_error = command_domain::SamplingSource::latest_from_subject("sensors.>").unwrap_err();
    assert!(matches!(
        invalid_error,
        command_domain::SamplingSubjectError::Invalid { .. }
    ));

    CommandExecution::new(&store, &command_base_schedule("alpha"))
        .execute()
        .await
        .unwrap();
    let same_state_error = CommandExecution::new(&store, &ResumeSchedule::new(command_schedule_id("alpha")))
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap_err();
    assert!(matches!(
        same_state_error,
        CommandError::Decide(crate::ResumeScheduleError::AlreadyActive { .. })
    ));

    let missing_error = CommandExecution::new(&store, &PauseSchedule::new(command_schedule_id("missing")))
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap_err();
    assert!(matches!(
        missing_error,
        CommandError::Decide(crate::PauseScheduleError::ScheduleNotFound { .. })
    ));
}

#[test]
fn ensure_write_condition_covers_accept_and_conflict_paths() {
    ScheduleWriteCondition::MustNotExist
        .ensure("alpha", ScheduleWriteState::new(None, false))
        .unwrap();
    ScheduleWriteCondition::MustBeAtPosition(position(3))
        .ensure("alpha", ScheduleWriteState::new(Some(position(3)), true))
        .unwrap();

    let error = ScheduleWriteCondition::MustNotExist
        .ensure("alpha", ScheduleWriteState::new(Some(position(4)), true))
        .unwrap_err();
    assert!(matches!(
        error,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: Some(_),
            ..
        }
    ));

    let error = ScheduleWriteCondition::MustBeAtPosition(position(3))
        .ensure("alpha", ScheduleWriteState::new(Some(position(4)), true))
        .unwrap_err();
    assert!(matches!(
        error,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: Some(_),
            ..
        }
    ));
}

fn created_event(
    id: &str,
    schedule: ScheduleEventSchedule,
    delivery: ScheduleEventDelivery,
    message: MessageEnvelope,
) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: id.to_string(),
                status: MessageField::some(v1::ScheduleStatus {
                    kind: Some(v1::schedule_status::Scheduled {}.into()),
                }),
                schedule: MessageField::some(proto_schedule(&schedule)),
                delivery: MessageField::some(proto_delivery(&delivery)),
                message: MessageField::some(proto_message(&message)),
            }
            .into(),
        ),
    }
}

fn lifecycle_event(case: ScheduleEventCase) -> v1::ScheduleEvent {
    v1::ScheduleEvent { event: Some(case) }
}

async fn append_events(
    store: &MockSchedulerStore,
    id: &str,
    events: &[v1::ScheduleEvent],
    precondition: StreamWritePrecondition,
) -> Result<(), SchedulerError> {
    let encoded = events.iter().map(encode_event).collect();
    store
        .append_stream(AppendStreamRequest {
            stream_id: id,
            stream_write_precondition: precondition,
            events: encoded,
        })
        .await
        .map(|_| ())
}

fn dt(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.timestamp_opt(seconds, 0).single().unwrap()
}

fn rich_delivery() -> ScheduleEventDelivery {
    ScheduleEventDelivery::NatsMessage {
        subject: "agent.run".to_string(),
        ttl: Some(std::time::Duration::from_secs(30)),
        source: Some(ScheduleEventSamplingSource::LatestFromSubject {
            subject: "sensors.temp".to_string(),
        }),
    }
}

fn rich_message() -> MessageEnvelope {
    MessageEnvelope {
        content: MessageContent::from_static("text/plain", "hi"),
        headers: MessageHeaders::from_pairs([("x-kind", "heartbeat")]),
    }
}

#[tokio::test]
async fn append_round_trips_every_schedule_kind() {
    let store = MockSchedulerStore::new();
    let kinds = [
        ("at", ScheduleEventSchedule::At { at: dt(1_700_000_000) }),
        (
            "cron",
            ScheduleEventSchedule::Cron {
                expr: "0 * * * *".to_string(),
                timezone: Some("UTC".to_string()),
            },
        ),
        (
            "rrule",
            ScheduleEventSchedule::RRule {
                dtstart: dt(1_700_000_000),
                rrule: "FREQ=DAILY".to_string(),
                timezone: Some("UTC".to_string()),
                rdate: vec![dt(1_700_000_100)],
                exdate: vec![dt(1_700_000_200)],
            },
        ),
    ];
    for (id, schedule) in kinds {
        append_events(
            &store,
            id,
            &[created_event(id, schedule.clone(), rich_delivery(), rich_message())],
            StreamWritePrecondition::NoStream,
        )
        .await
        .unwrap();
        let got = store
            .get_schedule(GetSchedule::new(ScheduleId::parse(id).unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.schedule, schedule, "{id}");
        assert!(matches!(
            got.delivery,
            ScheduleEventDelivery::NatsMessage {
                ttl: Some(_),
                source: Some(_),
                ..
            }
        ));
        assert_eq!(got.message.headers.as_slice().len(), 1);
    }
}

#[tokio::test]
async fn append_projects_full_lifecycle_and_reads_stream() {
    let store = MockSchedulerStore::new();
    let id = "alpha";
    append_events(
        &store,
        id,
        &[created_event(
            id,
            ScheduleEventSchedule::Every {
                every: std::time::Duration::from_secs(30),
            },
            rich_delivery(),
            rich_message(),
        )],
        StreamWritePrecondition::NoStream,
    )
    .await
    .unwrap();

    append_events(
        &store,
        id,
        &[
            lifecycle_event(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::none(),
                    scheduled_at: MessageField::none(),
                }
                .into(),
            ),
            lifecycle_event(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::none(),
                    recorded_at: MessageField::none(),
                }
                .into(),
            ),
            lifecycle_event(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
            lifecycle_event(
                v1::ScheduleResumed {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
            lifecycle_event(
                v1::ScheduleCompleted {
                    schedule_id: id.to_string(),
                    last_occurrence_sequence: Some(1),
                }
                .into(),
            ),
        ],
        StreamWritePrecondition::StreamExists,
    )
    .await
    .unwrap();

    let completed = store
        .get_schedule(GetSchedule::new(ScheduleId::parse(id).unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert!(completed.completed);

    let from_start = store
        .read_stream(ReadStreamRequest {
            stream_id: id,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert!(from_start.events.len() >= 6);
    let from_position = store
        .read_stream(ReadStreamRequest {
            stream_id: id,
            from: ReadFrom::Position(position(3)),
        })
        .await
        .unwrap();
    assert!(from_position.events.len() < from_start.events.len());

    append_events(
        &store,
        id,
        &[lifecycle_event(
            v1::ScheduleRemoved {
                schedule_id: id.to_string(),
            }
            .into(),
        )],
        StreamWritePrecondition::StreamExists,
    )
    .await
    .unwrap();
    assert!(
        store
            .get_schedule(GetSchedule::new(ScheduleId::parse(id).unwrap()))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn append_enforces_write_preconditions() {
    let store = MockSchedulerStore::new();
    let id = "beta";
    let create = created_event(
        id,
        ScheduleEventSchedule::Every {
            every: std::time::Duration::from_secs(30),
        },
        rich_delivery(),
        rich_message(),
    );

    assert!(matches!(
        append_events(
            &store,
            id,
            std::slice::from_ref(&create),
            StreamWritePrecondition::StreamExists
        )
        .await
        .unwrap_err(),
        SchedulerError::OptimisticConcurrencyConflict { .. }
    ));

    append_events(&store, id, std::slice::from_ref(&create), StreamWritePrecondition::Any)
        .await
        .unwrap();

    assert!(
        append_events(
            &store,
            id,
            std::slice::from_ref(&create),
            StreamWritePrecondition::NoStream
        )
        .await
        .is_err()
    );
    assert!(
        append_events(
            &store,
            id,
            std::slice::from_ref(&create),
            StreamWritePrecondition::At(position(99))
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn append_pause_without_prior_create_is_an_error() {
    let store = MockSchedulerStore::new();
    let result = append_events(
        &store,
        "ghost",
        &[lifecycle_event(
            v1::SchedulePaused {
                schedule_id: "ghost".to_string(),
            }
            .into(),
        )],
        StreamWritePrecondition::Any,
    )
    .await;
    assert!(result.is_err());
}
