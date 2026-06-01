#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_decider_runtime::{CommandExecution, ImmediateSnapshotTaskScheduler, StreamPosition};
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, ListSchedulesCommand, MessageContent, MessageEnvelope, MessageHeaders,
    PauseSchedule, RemoveSchedule, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
    ScheduleId, ScheduleWriteCondition, commands::domain as command_domain, mocks::MockSchedulerStore,
};

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

fn command_schedule_id(id: &str) -> command_domain::ScheduleId {
    command_domain::ScheduleId::parse(id).unwrap()
}

fn expected_schedule(id: &str) -> Schedule {
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
            // Matches the command domain's default content type (octet-stream).
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

#[tokio::test]
async fn client_register_then_get() {
    let store = MockSchedulerStore::new();

    let job = command_base_schedule("backup");
    CommandExecution::new(&store, &job).execute().await.unwrap();

    let got = store
        .get_schedule(GetScheduleCommand::new(ScheduleId::parse("backup").unwrap()))
        .await
        .unwrap();
    assert_eq!(got, Some(expected_schedule("backup")));
}

#[tokio::test]
async fn client_pause_job_toggles_job() {
    let store = MockSchedulerStore::new();

    CommandExecution::new(&store, &command_base_schedule("toggle"))
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store, &PauseSchedule::new(command_schedule_id("toggle")))
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();

    let got = store
        .get_schedule(GetScheduleCommand::new(ScheduleId::parse("toggle").unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status, ScheduleEventStatus::Paused);
}

#[tokio::test]
async fn client_remove_and_list_schedules_use_store_paths() {
    let store = MockSchedulerStore::new();

    CommandExecution::new(&store, &command_base_schedule("alpha"))
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store, &command_base_schedule("beta"))
        .execute()
        .await
        .unwrap();

    let listed = store.list_schedules(ListSchedulesCommand).await.unwrap();
    assert_eq!(listed.len(), 2);

    CommandExecution::new(&store, &RemoveSchedule::new(command_schedule_id("beta")))
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();

    assert!(
        store
            .get_schedule(GetScheduleCommand::new(ScheduleId::parse("beta").unwrap()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.list_schedules(ListSchedulesCommand).await.unwrap().len(), 1);
}

#[tokio::test]
async fn client_rejects_invalid_route() {
    let error = command_domain::Delivery::nats_event("agent.>").unwrap_err();

    assert!(matches!(error, command_domain::DeliveryRouteError::Invalid { .. }));
}

#[tokio::test]
async fn client_rejects_invalid_source_subject() {
    let error = command_domain::SamplingSource::latest_from_subject("sensors.>").unwrap_err();

    assert!(matches!(error, command_domain::SamplingSubjectError::Invalid { .. }));
}

#[tokio::test]
async fn client_rejects_stale_version() {
    let error = ScheduleWriteCondition::MustBeAtPosition(position(99))
        .ensure(
            "stale",
            trogon_scheduler::config::ScheduleWriteState::new(Some(position(1)), true),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        trogon_scheduler::SchedulerError::OptimisticConcurrencyConflict { .. }
    ));
}
