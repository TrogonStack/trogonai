#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_cron::{
    AddJobCommand, CronController, CronJob, GetJobCommand, JobEventDelivery, JobEventSchedule, JobEventStatus, JobId,
    JobWriteCondition, ListJobsCommand, MessageContent, MessageEnvelope, MessageHeaders, PauseJobCommand,
    RemoveJobCommand, SchedulePublisher,
    commands::domain as command_domain,
    mocks::{MockCronStore, MockLeaderLock, MockSchedulePublisher},
};
use trogon_eventsourcing::{CommandExecution, StreamPosition, run_task_immediately};

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

fn command_job_id(id: &str) -> command_domain::JobId {
    command_domain::JobId::parse(id).unwrap()
}

fn expected_job(id: &str) -> CronJob {
    CronJob {
        id: id.to_string(),
        status: JobEventStatus::Enabled,
        schedule: JobEventSchedule::Every { every_sec: 30 },
        delivery: JobEventDelivery::NatsEvent {
            route: "agent.run".to_string(),
            ttl_sec: None,
            source: None,
        },
        message: MessageEnvelope {
            content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        },
    }
}

fn command_base_job(id: &str) -> command_domain::Job {
    command_domain::Job {
        id: command_job_id(id),
        status: command_domain::JobStatus::Enabled,
        schedule: command_domain::Schedule::every(30).unwrap(),
        delivery: command_domain::Delivery::nats_event("agent.run").unwrap(),
        message: command_domain::JobMessage {
            content: command_domain::MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: command_domain::JobHeaders::default(),
        },
    }
}

#[tokio::test]
async fn client_register_then_get() {
    let store = MockCronStore::new();

    let job = command_base_job("backup");
    CommandExecution::new(&store, &AddJobCommand::new(job))
        .with_snapshot(&store)
        .with_task_runtime(run_task_immediately)
        .execute()
        .await
        .unwrap();

    let got = store
        .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
        .await
        .unwrap();
    assert_eq!(got, Some(expected_job("backup")));
}

#[tokio::test]
async fn client_pause_job_toggles_job() {
    let store = MockCronStore::new();

    CommandExecution::new(&store, &AddJobCommand::new(command_base_job("toggle")))
        .with_snapshot(&store)
        .with_task_runtime(run_task_immediately)
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store, &PauseJobCommand::new(command_job_id("toggle")))
        .with_snapshot(&store)
        .with_task_runtime(run_task_immediately)
        .execute()
        .await
        .unwrap();

    let got = store
        .get_job(GetJobCommand::new(JobId::parse("toggle").unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status, JobEventStatus::Disabled);
}

#[tokio::test]
async fn client_remove_and_list_jobs_use_store_paths() {
    let store = MockCronStore::new();

    CommandExecution::new(&store, &AddJobCommand::new(command_base_job("alpha")))
        .with_snapshot(&store)
        .with_task_runtime(run_task_immediately)
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store, &AddJobCommand::new(command_base_job("beta")))
        .with_snapshot(&store)
        .with_task_runtime(run_task_immediately)
        .execute()
        .await
        .unwrap();

    let listed = store.list_jobs(ListJobsCommand).await.unwrap();
    assert_eq!(listed.len(), 2);

    CommandExecution::new(&store, &RemoveJobCommand::new(command_job_id("beta")))
        .with_snapshot(&store)
        .with_task_runtime(run_task_immediately)
        .execute()
        .await
        .unwrap();

    assert!(
        store
            .get_job(GetJobCommand::new(JobId::parse("beta").unwrap()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.list_jobs(ListJobsCommand).await.unwrap().len(), 1);
}

#[tokio::test]
async fn client_rejects_invalid_route() {
    let error = serde_json::from_value::<command_domain::Job>(serde_json::json!({
        "id": "bad",
        "schedule": { "type": "every", "every_sec": 30 },
        "delivery": { "type": "nats_event", "route": "agent.>" },
        "content": "{\"kind\":\"heartbeat\"}"
    }))
    .unwrap_err();

    assert!(error.to_string().contains("route"));
}

#[tokio::test]
async fn client_rejects_invalid_source_subject() {
    let error = serde_json::from_value::<command_domain::Job>(serde_json::json!({
        "id": "bad-source",
        "schedule": { "type": "every", "every_sec": 30 },
        "delivery": {
            "type": "nats_event",
            "route": "agent.run",
            "source": { "type": "latest_from_subject", "subject": "sensors.>" }
        },
        "content": "{\"kind\":\"heartbeat\"}"
    }))
    .unwrap_err();

    assert!(error.to_string().contains("sampling source"));
}

#[tokio::test]
async fn client_rejects_stale_version() {
    let error = JobWriteCondition::MustBeAtPosition(position(99))
        .ensure(
            "stale",
            trogon_cron::config::JobWriteState::new(Some(position(1)), true),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        trogon_cron::CronError::OptimisticConcurrencyConflict { .. }
    ));
}

#[tokio::test]
async fn mock_schedule_publisher_records_changes() {
    let publisher = MockSchedulePublisher::new();
    let details = trogon_cron::v1::JobDetails::from(&command_base_job("alpha"));
    let resolved = trogon_cron::ResolvedJob::from_event("alpha", details.as_view()).unwrap();

    publisher.upsert_schedule(&resolved).await.unwrap();
    publisher.remove_schedule("alpha").await.unwrap();

    assert_eq!(publisher.upserts(), vec!["cron.schedules.alpha"]);
    assert_eq!(publisher.removals(), vec!["alpha"]);
}

#[test]
fn controller_new_with_mocks_compiles() {
    let _controller = CronController::new(
        MockCronStore::new(),
        MockSchedulePublisher::new(),
        MockLeaderLock::new(),
    );
}
