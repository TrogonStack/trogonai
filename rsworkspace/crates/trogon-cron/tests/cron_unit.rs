use std::collections::BTreeMap;

use trogon_cron::{
    ConfigStore, CronController, DeliverySpec, JobEnabledState, JobSpec, JobWriteCondition,
    SamplingSource, SchedulePublisher, ScheduleSpec,
    mocks::{MockConfigStore, MockLeaderLock, MockSchedulePublisher},
};

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
    let store = MockConfigStore::new();

    let job = base_job("backup");
    store
        .put_job(&job, JobWriteCondition::MustNotExist)
        .await
        .unwrap();

    let got = store.get_job("backup").await.unwrap();
    assert_eq!(got.map(|job| job.spec), Some(job));
}

#[tokio::test]
async fn client_set_enabled_toggles_job() {
    let store = MockConfigStore::new();

    store
        .put_job(&base_job("toggle"), JobWriteCondition::MustNotExist)
        .await
        .unwrap();
    let version = store.get_job("toggle").await.unwrap().unwrap().version;
    store
        .set_job_state(
            "toggle",
            JobEnabledState::Disabled,
            JobWriteCondition::MustBeAtVersion(version),
        )
        .await
        .unwrap();

    let got = store.get_job("toggle").await.unwrap().unwrap();
    assert_eq!(got.spec.state, JobEnabledState::Disabled);
}

#[tokio::test]
async fn client_remove_and_list_jobs_use_store_paths() {
    let store = MockConfigStore::new();

    store
        .put_job(&base_job("alpha"), JobWriteCondition::MustNotExist)
        .await
        .unwrap();
    store
        .put_job(&base_job("beta"), JobWriteCondition::MustNotExist)
        .await
        .unwrap();

    let listed = store.list_jobs().await.unwrap();
    assert_eq!(listed.len(), 2);

    let beta_version = store.get_job("beta").await.unwrap().unwrap().version;
    store
        .delete_job("beta", JobWriteCondition::MustBeAtVersion(beta_version))
        .await
        .unwrap();

    assert!(store.get_job("beta").await.unwrap().is_none());
    assert_eq!(store.list_jobs().await.unwrap().len(), 1);
}

#[tokio::test]
async fn client_rejects_invalid_route() {
    let store = MockConfigStore::new();
    let mut job = base_job("bad");
    job.delivery = DeliverySpec::NatsEvent {
        route: "agent.>".to_string(),
        headers: BTreeMap::new(),
        ttl_sec: None,
        source: None,
    };

    let error = store
        .put_job(&job, JobWriteCondition::MustNotExist)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("route"));
}

#[tokio::test]
async fn client_rejects_invalid_source_subject() {
    let store = MockConfigStore::new();
    let mut job = base_job("bad-source");
    job.delivery = DeliverySpec::NatsEvent {
        route: "agent.run".to_string(),
        headers: BTreeMap::new(),
        ttl_sec: None,
        source: Some(SamplingSource::LatestFromSubject {
            subject: "sensors.>".to_string(),
        }),
    };

    let error = store
        .put_job(&job, JobWriteCondition::MustNotExist)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("sampling source"));
}

#[tokio::test]
async fn client_rejects_stale_version() {
    let store = MockConfigStore::new();
    store
        .put_job(&base_job("stale"), JobWriteCondition::MustNotExist)
        .await
        .unwrap();

    let error = store
        .set_job_state(
            "stale",
            JobEnabledState::Disabled,
            JobWriteCondition::MustBeAtVersion(99),
        )
        .await
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
        MockConfigStore::new(),
        MockSchedulePublisher::new(),
        MockLeaderLock::new(),
    );
}
