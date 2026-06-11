#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use buffa::MessageField;
use trogon_decider_runtime::{CommandExecution, ImmediateSnapshotTaskScheduler, StreamPosition};
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, ListSchedulesCommand, MessageContent, MessageEnvelope, MessageHeaders,
    PauseSchedule, RemoveSchedule, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
    ScheduleId, SchedulePublisher, ScheduleWriteCondition, SchedulerController,
    commands::domain as command_domain,
    mocks::{MockLeaderLock, MockSchedulePublisher, MockSchedulerStore},
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
        schedule: ScheduleEventSchedule::Every { every_sec: 30 },
        delivery: ScheduleEventDelivery::NatsMessage {
            subject: "agent.run".to_string(),
            ttl_sec: None,
            source: None,
        },
        message: MessageEnvelope {
            content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
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

    assert!(error.to_string().contains("route"));
}

#[tokio::test]
async fn client_rejects_invalid_source_subject() {
    let error = command_domain::SamplingSource::latest_from_subject("sensors.>").unwrap_err();

    assert!(error.to_string().contains("sampling subject"));
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

#[tokio::test]
async fn mock_schedule_publisher_records_changes() {
    use buffa_types::google::protobuf::Duration;
    let publisher = MockSchedulePublisher::new();
    let job = trogon_scheduler::v1::ScheduleCreated {
        schedule_id: "alpha".to_string(),
        status: MessageField::some(trogon_scheduler::v1::ScheduleStatus {
            kind: Some(trogon_scheduler::v1::schedule_status::Scheduled {}.into()),
        }),
        schedule: MessageField::some(trogon_scheduler::v1::Schedule {
            kind: Some(
                trogon_scheduler::v1::schedule::Every {
                    every: MessageField::some(Duration {
                        seconds: 30,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }),
        delivery: MessageField::some(trogon_scheduler::v1::Delivery {
            kind: Some(
                trogon_scheduler::v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ttl: MessageField::none(),
                    source: MessageField::none(),
                }
                .into(),
            ),
        }),
        message: MessageField::some(trogon_scheduler::v1::Message {
            content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                content_type: "application/json".to_string(),
                data: r#"{"kind":"heartbeat"}"#.as_bytes().to_vec(),
            }),
            headers: Vec::new(),
        }),
    };
    let resolved = trogon_scheduler::ResolvedSchedule::from_event("alpha", &job).unwrap();

    publisher.upsert_schedule(&resolved).await.unwrap();
    publisher.remove_schedule("alpha").await.unwrap();

    assert_eq!(publisher.upserts(), vec!["scheduler.schedules.alpha"]);
    assert_eq!(publisher.removals(), vec!["alpha"]);
}

#[test]
fn controller_new_with_mocks_compiles() {
    let _controller = SchedulerController::new(
        MockSchedulerStore::new(),
        MockSchedulePublisher::new(),
        MockLeaderLock::new(),
    );
}
