#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_cron::{
    AddJobCommand, CronController, CronJob, Delivery, GetJobCommand, Job, JobDetails, JobEventStatus, JobHeaders,
    JobId, JobMessage, JobStatus, JobWriteCondition, ListJobsCommand, MessageContent, PauseJobCommand,
    RemoveJobCommand, Schedule, SchedulePublisher,
    mocks::{MockCronStore, MockLeaderLock, MockSchedulePublisher},
};
use trogon_eventsourcing::CommandExecution;

fn job_id(id: &str) -> JobId {
    JobId::parse(id).unwrap()
}

fn base_job(id: &str) -> Job {
    Job {
        id: job_id(id),
        status: JobStatus::Enabled,
        schedule: Schedule::every(30).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: JobMessage {
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: JobHeaders::default(),
        },
    }
}

fn expected_job(id: &str) -> CronJob {
    CronJob::from((id.to_string(), JobDetails::from(base_job(id))))
}

#[tokio::test]
async fn client_register_then_get() {
    let store = MockCronStore::new();

    let job = base_job("backup");
    CommandExecution::new(&store, &AddJobCommand::new(job))
        .with_snapshot(&store)
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

    CommandExecution::new(&store, &AddJobCommand::new(base_job("toggle")))
        .with_snapshot(&store)
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store, &PauseJobCommand::new(job_id("toggle")))
        .with_snapshot(&store)
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

    CommandExecution::new(&store, &AddJobCommand::new(base_job("alpha")))
        .with_snapshot(&store)
        .execute()
        .await
        .unwrap();
    CommandExecution::new(&store, &AddJobCommand::new(base_job("beta")))
        .with_snapshot(&store)
        .execute()
        .await
        .unwrap();

    let listed = store.list_jobs(ListJobsCommand).await.unwrap();
    assert_eq!(listed.len(), 2);

    CommandExecution::new(&store, &RemoveJobCommand::new(job_id("beta")))
        .with_snapshot(&store)
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
    let error = serde_json::from_value::<Job>(serde_json::json!({
        "id": "bad",
        "schedule": { "type": "every", "every_sec": 30 },
        "delivery": { "type": "nats_event", "route": "agent.>" },
        "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
    }))
    .unwrap_err();

    assert!(error.to_string().contains("route"));
}

#[tokio::test]
async fn client_rejects_invalid_source_subject() {
    let error = serde_json::from_value::<Job>(serde_json::json!({
        "id": "bad-source",
        "schedule": { "type": "every", "every_sec": 30 },
        "delivery": {
            "type": "nats_event",
            "route": "agent.run",
            "source": { "type": "latest_from_subject", "subject": "sensors.>" }
        },
        "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
    }))
    .unwrap_err();

    assert!(error.to_string().contains("sampling source"));
}

#[tokio::test]
async fn client_rejects_stale_version() {
    let error = JobWriteCondition::MustBeAtVersion(99)
        .ensure("stale", trogon_cron::config::JobWriteState::new(Some(1), true))
        .unwrap_err();

    assert!(matches!(
        error,
        trogon_cron::CronError::OptimisticConcurrencyConflict { .. }
    ));
}

#[tokio::test]
async fn mock_schedule_publisher_records_changes() {
    let publisher = MockSchedulePublisher::new();
    let resolved = trogon_cron::ResolvedJob::try_from(&expected_job("alpha")).unwrap();

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
