#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_cron::{
    AddJobCommand, CronController, CronJob, DeliverySpec, GetJobCommand, JobDetails,
    JobEnabledState, JobEventState, JobHeaders, JobId, JobSpec, JobWriteCondition, ListJobsCommand,
    MessageContent, PauseJobCommand, RemoveJobCommand, SchedulePublisher, ScheduleSpec, add_job,
    mocks::{MockCronStore, MockLeaderLock, MockSchedulePublisher},
    pause_job, remove_job,
};

fn job_id(id: &str) -> JobId {
    JobId::parse(id).unwrap()
}

fn base_job(id: &str) -> JobSpec {
    JobSpec {
        id: job_id(id),
        state: JobEnabledState::Enabled,
        schedule: ScheduleSpec::every(30).unwrap(),
        delivery: DeliverySpec::nats_event("agent.run").unwrap(),
        content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
        headers: JobHeaders::default(),
    }
}

fn expected_job(id: &str) -> CronJob {
    CronJob::from((id.to_string(), JobDetails::from(base_job(id))))
}

#[tokio::test]
async fn client_register_then_get() {
    let store = MockCronStore::new();

    let job = base_job("backup");
    add_job(&store, AddJobCommand::new(job), None)
        .await
        .unwrap();

    let got = store
        .get_job(GetJobCommand {
            id: "backup".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(got, Some(expected_job("backup")));
}

#[tokio::test]
async fn client_pause_job_toggles_job() {
    let store = MockCronStore::new();

    add_job(&store, AddJobCommand::new(base_job("toggle")), None)
        .await
        .unwrap();
    pause_job(&store, PauseJobCommand::new(job_id("toggle")), None)
        .await
        .unwrap();

    let got = store
        .get_job(GetJobCommand {
            id: "toggle".to_string(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.state, JobEventState::Disabled);
}

#[tokio::test]
async fn client_remove_and_list_jobs_use_store_paths() {
    let store = MockCronStore::new();

    add_job(&store, AddJobCommand::new(base_job("alpha")), None)
        .await
        .unwrap();
    add_job(&store, AddJobCommand::new(base_job("beta")), None)
        .await
        .unwrap();

    let listed = store.list_jobs(ListJobsCommand).await.unwrap();
    assert_eq!(listed.len(), 2);

    remove_job(&store, RemoveJobCommand::new(job_id("beta")), None)
        .await
        .unwrap();

    assert!(
        store
            .get_job(GetJobCommand {
                id: "beta".to_string(),
            })
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.list_jobs(ListJobsCommand).await.unwrap().len(), 1);
}

#[tokio::test]
async fn client_rejects_invalid_route() {
    let error = serde_json::from_value::<JobSpec>(serde_json::json!({
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
    let error = serde_json::from_value::<JobSpec>(serde_json::json!({
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
        .ensure(
            "stale",
            trogon_cron::config::JobWriteState::new(Some(1), true),
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
    let resolved = trogon_cron::ResolvedJobSpec::try_from(&expected_job("alpha")).unwrap();

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
