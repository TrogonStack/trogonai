#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use trogon_cron::{
    ChangeJobStateCommand, CronController, DeliverySpec, GetJobCommand, JobEnabledState, JobId,
    JobSpec, JobWriteCondition, ListJobsCommand, OccPolicy, RegisterJobCommand, RemoveJobCommand,
    SamplingSource, SchedulePublisher, ScheduleSpec, change_job_state,
    mocks::{MockCronStore, MockLeaderLock, MockSchedulePublisher},
    register_job, remove_job,
};

fn job_id(id: &str) -> JobId {
    JobId::parse(id).unwrap()
}

fn base_job(id: &str) -> JobSpec {
    JobSpec {
        id: id.to_string(),
        state: JobEnabledState::Enabled,
        schedule: ScheduleSpec::Every { every_sec: 30 },
        delivery: DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: None,
        },
        payload: serde_json::json!({"kind": "heartbeat"}),
        metadata: BTreeMap::new(),
    }
}

#[tokio::test]
async fn client_register_then_get() {
    let store = MockCronStore::new();

    let job = base_job("backup");
    register_job(
        &store,
        &store,
        RegisterJobCommand::new(job).unwrap(),
        OccPolicy::CommandDefault,
    )
    .await
    .unwrap();

    let got = store
        .get_job(GetJobCommand {
            id: job_id("backup"),
        })
        .await
        .unwrap();
    assert_eq!(got.map(|job| job.payload), Some(base_job("backup")));
}

#[tokio::test]
async fn client_set_enabled_toggles_job() {
    let store = MockCronStore::new();

    register_job(
        &store,
        &store,
        RegisterJobCommand::new(base_job("toggle")).unwrap(),
        OccPolicy::CommandDefault,
    )
    .await
    .unwrap();
    change_job_state(
        &store,
        &store,
        ChangeJobStateCommand::new(job_id("toggle"), JobEnabledState::Disabled),
        OccPolicy::CommandDefault,
    )
    .await
    .unwrap();

    let got = store
        .get_job(GetJobCommand {
            id: job_id("toggle"),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.payload.state, JobEnabledState::Disabled);
}

#[tokio::test]
async fn client_remove_and_list_jobs_use_store_paths() {
    let store = MockCronStore::new();

    register_job(
        &store,
        &store,
        RegisterJobCommand::new(base_job("alpha")).unwrap(),
        OccPolicy::CommandDefault,
    )
    .await
    .unwrap();
    register_job(
        &store,
        &store,
        RegisterJobCommand::new(base_job("beta")).unwrap(),
        OccPolicy::CommandDefault,
    )
    .await
    .unwrap();

    let listed = store.list_jobs(ListJobsCommand).await.unwrap();
    assert_eq!(listed.len(), 2);

    remove_job(
        &store,
        &store,
        RemoveJobCommand::new(job_id("beta")),
        OccPolicy::CommandDefault,
    )
    .await
    .unwrap();

    assert!(
        store
            .get_job(GetJobCommand { id: job_id("beta") })
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.list_jobs(ListJobsCommand).await.unwrap().len(), 1);
}

#[tokio::test]
async fn client_rejects_invalid_route() {
    let mut job = base_job("bad");
    job.delivery = DeliverySpec::NatsEvent {
        route: "agent.>".to_string(),
        headers: BTreeMap::new(),
        ttl_sec: None,
        source: None,
    };

    let error = RegisterJobCommand::new(job).unwrap_err();
    assert!(error.to_string().contains("route"));
}

#[tokio::test]
async fn client_rejects_invalid_source_subject() {
    let mut job = base_job("bad-source");
    job.delivery = DeliverySpec::NatsEvent {
        route: "agent.run".to_string(),
        headers: BTreeMap::new(),
        ttl_sec: None,
        source: Some(SamplingSource::LatestFromSubject {
            subject: "sensors.>".to_string(),
        }),
    };

    let error = RegisterJobCommand::new(job).unwrap_err();
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
    let resolved = trogon_cron::ResolvedJobSpec::try_from(&base_job("alpha")).unwrap();

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
