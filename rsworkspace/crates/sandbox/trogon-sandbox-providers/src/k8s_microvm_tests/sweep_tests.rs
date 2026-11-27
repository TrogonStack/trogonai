use super::*;

#[tokio::test]
async fn a_sweep_recovers_a_live_lease_job_with_its_running_pod() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, lease_job("job-live", "lease-1", 30, 300)?);
    mock.seed_pod(job_pod("job-live-abcde", "Running", running()));

    let sweep = provider(mock.clone()).sweep_lease_jobs().await?;

    assert_eq!(
        sweep.live,
        vec![RecoveredLeaseJob {
            lease_id: "lease-1".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            job_name: "job-live".to_string(),
            pod_name: "job-live-abcde".to_string(),
        }]
    );
    assert!(sweep.swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_sweep_tears_down_an_expired_lease_job_and_its_derived_objects() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, lease_job("job-old", "lease-1", 301, 300)?);

    let sweep = provider(mock.clone()).sweep_lease_jobs().await?;

    assert_eq!(sweep.swept, vec!["job-old".to_string()]);
    assert!(sweep.live.is_empty());
    let deleted = deleted_names(&mock);
    for name in lease_job_secret_names("job-old").into_iter().chain([
        crate::k8s_network_policy::network_policy_name("job-old"),
        "job-old".to_string(),
    ]) {
        assert!(deleted.contains(&name), "{name} not deleted: {deleted:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_sweep_tears_down_a_finished_lease_job_even_without_a_clock() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    *mock.list_clock.locked() = None;
    seed_job(&mock, finished(lease_job("job-done", "lease-1", 5, 300)?, "Failed"));
    seed_job(&mock, lease_job("job-stale", "lease-2", 10_000, 300)?);

    let sweep = provider(mock.clone()).sweep_lease_jobs().await?;

    // Without the apiserver clock the stale Job's age is unknown, so it
    // is left alone rather than judged on a worker host's clock.
    assert_eq!(sweep.swept, vec!["job-done".to_string()]);
    assert!(!deleted_names(&mock).contains(&"job-stale".to_string()));
    Ok(())
}

#[tokio::test]
async fn a_sweep_ignores_jobs_being_deleted_or_without_a_lease() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut deleting = lease_job("job-deleting", "lease-1", 10_000, 300)?;
    deleting.metadata.deletion_timestamp = Some(Time(mock_clock()));
    seed_job(&mock, deleting);
    let mut unleased = lease_job("job-unleased", "lease-2", 10_000, 300)?;
    unleased.metadata.labels = None;
    seed_job(&mock, unleased);

    let sweep = provider(mock.clone()).sweep_lease_jobs().await?;

    assert_eq!(sweep, LeaseJobSweep::default());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_sweep_leaves_a_lease_job_whose_pod_is_not_running_yet() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, lease_job("job-new", "lease-1", 1, 300)?);
    mock.seed_pod(job_pod("job-new-abcde", "Pending", waiting("ContainerCreating")));

    let sweep = provider(mock.clone()).sweep_lease_jobs().await?;

    assert_eq!(sweep, LeaseJobSweep::default());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn the_job_owns_every_secret_its_launch_created() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    provider(mock.clone())
        .launch_with_options(
            sample_spec_with_clone(EgressPolicy::DenyAll)?,
            K8sLaunchOptions {
                restore_secret_name: Some("job-restore".to_string()),
                s3_volume_secret_name: Some("job-s3-volume".to_string()),
                keep_alive: true,
                ..Default::default()
            },
        )
        .await?;

    let job_name = mock.created_jobs()[0].metadata.name.clone().ok_or("missing value")?;
    let mut owned: Vec<String> = mock
        .secret_owners()
        .into_iter()
        .map(|(secret, owner)| {
            assert_eq!(owner.kind, "Job");
            assert_eq!(owner.api_version, "batch/v1");
            assert_eq!(owner.name, job_name);
            assert_eq!(owner.uid, format!("uid-{job_name}"));
            secret
        })
        .collect();
    owned.sort();
    let mut expected = vec![
        clone_secret_name(&job_name),
        "job-restore".to_string(),
        "job-s3-volume".to_string(),
    ];
    expected.sort();
    assert_eq!(owned, expected);
    Ok(())
}

#[test]
fn a_config_without_an_identity_owns_every_lease_resource() -> TestResult {
    let config = K8sMicroVmConfig::new("default");
    let other = BTreeMap::from([(LABEL_WORKER_IDENTITY.to_string(), "w-1".to_string())]);
    assert!(config.owns_lease_resource(&other));
    assert!(config.owns_lease_resource(&BTreeMap::new()));
    Ok(())
}

#[tokio::test]
async fn a_replica_recovers_its_own_and_unlabeled_lease_jobs_but_not_another_replicas() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, owned_by(lease_job("job-mine", "lease-mine", 30, 300)?, "w-0"));
    seed_job(
        &mock,
        owned_by(lease_job("job-theirs", "lease-theirs", 30, 300)?, "w-1"),
    );
    seed_job(&mock, lease_job("job-legacy", "lease-legacy", 30, 300)?);
    for job in ["job-mine", "job-theirs", "job-legacy"] {
        mock.seed_pod(job_pod(&format!("{job}-abcde"), "Running", running()));
    }

    let sweep = replica(mock.clone(), "w-0", true).sweep_lease_jobs().await?;

    assert_eq!(
        live_lease_ids(&sweep),
        vec!["lease-legacy".to_string(), "lease-mine".to_string()]
    );
    Ok(())
}

#[tokio::test]
async fn a_replica_skips_unlabeled_lease_jobs_once_told_to() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, owned_by(lease_job("job-mine", "lease-mine", 30, 300)?, "w-0"));
    seed_job(&mock, lease_job("job-legacy", "lease-legacy", 30, 300)?);
    for job in ["job-mine", "job-legacy"] {
        mock.seed_pod(job_pod(&format!("{job}-abcde"), "Running", running()));
    }

    let sweep = replica(mock.clone(), "w-0", false).sweep_lease_jobs().await?;

    assert_eq!(live_lease_ids(&sweep), vec!["lease-mine".to_string()]);
    Ok(())
}

#[tokio::test]
async fn a_replica_reclaims_a_lease_job_its_owner_abandoned_past_the_grace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(
        &mock,
        owned_by(
            lease_job("job-dead", "lease-1", 300 + FOREIGN_LEASE_GRACE_SECONDS, 300)?,
            "w-1",
        ),
    );

    let sweep = replica(mock.clone(), "w-0", true).sweep_lease_jobs().await?;

    assert_eq!(sweep.swept, vec!["job-dead".to_string()]);
    assert!(deleted_names(&mock).contains(&"job-dead".to_string()));
    Ok(())
}

#[tokio::test]
async fn a_replica_leaves_another_replicas_finished_lease_job_alone() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(
        &mock,
        owned_by(
            finished(lease_job("job-theirs", "lease-1", 10, 300)?, "Complete"),
            "w-1",
        ),
    );

    let sweep = replica(mock.clone(), "w-0", true).sweep_lease_jobs().await?;

    assert!(sweep.swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_replica_leaves_another_replicas_lease_job_alone_within_the_grace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, owned_by(lease_job("job-theirs", "lease-1", 301, 300)?, "w-1"));

    let sweep = replica(mock.clone(), "w-0", true).sweep_lease_jobs().await?;

    assert!(sweep.swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_replica_labels_the_lease_jobs_it_launches() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    replica(mock.clone(), "w-0", true)
        .launch_with_options(
            sample_spec(EgressPolicy::DenyAll)?,
            K8sLaunchOptions {
                lease_id: Some("lease-a".to_string()),
                ..Default::default()
            },
        )
        .await?;

    let jobs = mock.created_jobs();
    let labels = jobs[0].metadata.labels.clone().ok_or("missing value")?;
    assert_eq!(labels[LABEL_WORKER_IDENTITY], "w-0");
    Ok(())
}

#[tokio::test]
async fn a_replica_labels_the_warm_pods_it_claims_and_recovers_only_those() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mine = replica(mock.clone(), "w-0", true);
    let pod_name = mine
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    mine.claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::ZERO)
        .await?
        .ok_or("missing value")?;

    let labels = mock.pod_named(&pod_name)?.metadata.labels.ok_or("missing value")?;
    assert_eq!(labels[LABEL_WORKER_IDENTITY], "w-0");
    assert_eq!(mine.claimed_warm_pods().await?.len(), 1);
    assert!(replica(mock.clone(), "w-1", true).claimed_warm_pods().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claim_stamps_the_lease_deadline_on_the_apiserver_clock() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mine = replica(mock.clone(), "w-0", true);
    let pod_name = mine
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    mine.claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::from_secs(600))
        .await?
        .ok_or("missing value")?;

    let labels = mock.pod_named(&pod_name)?.metadata.labels.ok_or("missing value")?;
    assert_eq!(
        labels[LABEL_LEASE_DEADLINE],
        (mock_clock().as_second() + 600).to_string()
    );
    Ok(())
}

#[tokio::test]
async fn a_claim_without_a_timeout_stamps_no_lease_deadline() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mine = replica(mock.clone(), "w-0", true);
    let pod_name = mine
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    mine.claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::ZERO)
        .await?
        .ok_or("missing value")?;

    let labels = mock.pod_named(&pod_name)?.metadata.labels.ok_or("missing value")?;
    assert!(!labels.contains_key(LABEL_LEASE_DEADLINE));
    Ok(())
}

#[tokio::test]
async fn a_claim_without_an_apiserver_clock_stamps_no_lease_deadline() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mine = replica(mock.clone(), "w-0", true);
    let pod_name = mine
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    *mock.list_clock.locked() = None;
    mine.claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::from_secs(600))
        .await?
        .ok_or("missing value")?;

    let labels = mock.pod_named(&pod_name)?.metadata.labels.ok_or("missing value")?;
    assert!(!labels.contains_key(LABEL_LEASE_DEADLINE));
    Ok(())
}

#[tokio::test]
async fn a_sweep_deletes_an_expired_claimed_warm_pod_with_its_secret_and_policy() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(claimed_warm_pod_due("wp-old", "lease-1", "w-0", Some(-1)));
    mock.seed_pod(claimed_warm_pod_due("wp-due", "lease-2", "w-0", Some(0)));
    mock.seed_pod(claimed_warm_pod_due("wp-young", "lease-3", "w-0", Some(60)));
    mock.seed_pod(claimed_warm_pod_due("wp-forever", "lease-4", "w-0", None));

    let mut swept = replica(mock.clone(), "w-0", true).sweep_claimed_warm_pods().await?;
    swept.sort();

    assert_eq!(swept, vec!["wp-due".to_string(), "wp-old".to_string()]);
    let deleted = deleted_names(&mock);
    for name in [
        "wp-old".to_string(),
        "wp-old-restore".to_string(),
        crate::k8s_network_policy::network_policy_name("wp-old"),
    ] {
        assert!(deleted.contains(&name), "{name} not deleted: {deleted:?}");
    }
    assert!(!deleted.contains(&"wp-young".to_string()));
    assert!(!deleted.contains(&"wp-forever".to_string()));
    Ok(())
}

#[tokio::test]
async fn a_replica_reclaims_a_claimed_warm_pod_its_owner_abandoned_past_the_grace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(claimed_warm_pod_due(
        "wp-dead",
        "lease-1",
        "w-1",
        Some(-FOREIGN_LEASE_GRACE_SECONDS),
    ));

    let swept = replica(mock.clone(), "w-0", true).sweep_claimed_warm_pods().await?;

    assert_eq!(swept, vec!["wp-dead".to_string()]);
    Ok(())
}

#[tokio::test]
async fn a_replica_leaves_another_replicas_expired_claimed_warm_pod_alone_within_the_grace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(claimed_warm_pod_due("wp-theirs", "lease-1", "w-1", Some(-1)));

    let swept = replica(mock.clone(), "w-0", true).sweep_claimed_warm_pods().await?;

    assert!(swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_no_deadline_job_of_a_removed_replica_is_reclaimed_once_absent_for_the_grace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("w-0"));
    mock.seed_pod(worker_pod("w-1"));
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("gone-old", "l1", ORPHAN_RECLAIM_GRACE_SECONDS + 1, 0)?)?,
            "w-2",
        ),
    );
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("gone-young", "l2", 60 - ORPHAN_RECLAIM_GRACE_SECONDS, 0)?)?,
            "w-2",
        ),
    );
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("alive-old", "l3", ORPHAN_RECLAIM_GRACE_SECONDS + 1, 0)?)?,
            "w-1",
        ),
    );
    let provider = replica(mock.clone(), "w-0", true);

    // The first sweep only notices that w-2 is missing.
    assert!(provider.sweep_lease_jobs().await?.swept.is_empty());
    for secs in sweep_times(ORPHAN_RECLAIM_GRACE_SECONDS - 1) {
        clock_at(&mock, secs)?;
        assert!(provider.sweep_lease_jobs().await?.swept.is_empty());
    }
    clock_at(&mock, ORPHAN_RECLAIM_GRACE_SECONDS)?;
    let sweep = provider.sweep_lease_jobs().await?;

    assert_eq!(sweep.swept, vec!["gone-old".to_string()]);
    let deleted = deleted_names(&mock);
    assert!(!deleted.contains(&"alive-old".to_string()));
    assert!(!deleted.contains(&"gone-young".to_string()));
    Ok(())
}

#[tokio::test]
async fn an_owner_missing_for_one_sweep_and_then_back_keeps_its_jobs() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("w-0"));
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("j", "l1", ORPHAN_RECLAIM_GRACE_SECONDS + 1, 0)?)?,
            "w-2",
        ),
    );
    let provider = replica(mock.clone(), "w-0", true);

    // w-2 is mid restart at the first sweep, back at the second, and gone
    // again at the third, so the absence never lasts the grace.
    provider.sweep_lease_jobs().await?;
    mock.seed_pod(worker_pod("w-2"));
    clock_at(&mock, 100)?;
    provider.sweep_lease_jobs().await?;
    remove_pod(&mock, "w-2");
    for secs in sweep_times(ORPHAN_RECLAIM_GRACE_SECONDS - 1) {
        clock_at(&mock, 100 + secs)?;
        assert!(provider.sweep_lease_jobs().await?.swept.is_empty());
    }
    assert!(mock.delete_calls().is_empty());

    clock_at(&mock, 100 + ORPHAN_RECLAIM_GRACE_SECONDS + 100)?;
    let sweep = provider.sweep_lease_jobs().await?;
    assert_eq!(sweep.swept, vec!["j".to_string()]);
    Ok(())
}

#[tokio::test]
async fn an_owner_pod_that_is_terminating_still_counts_as_there() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("w-0"));
    let mut owner = worker_pod("w-2");
    owner.metadata.deletion_timestamp = Some(Time(mock_clock()));
    mock.seed_pod(owner);
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("j", "l1", ORPHAN_RECLAIM_GRACE_SECONDS + 1, 0)?)?,
            "w-2",
        ),
    );
    let provider = replica(mock.clone(), "w-0", true);

    for secs in sweep_times(2 * ORPHAN_RECLAIM_GRACE_SECONDS) {
        clock_at(&mock, secs)?;
        assert!(provider.sweep_lease_jobs().await?.swept.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn orphan_reclaim_does_nothing_unless_this_replica_is_itself_a_visible_pod() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("someone-else"));
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("j", "l1", ORPHAN_RECLAIM_GRACE_SECONDS + 1, 0)?)?,
            "w-2",
        ),
    );

    let sweep = replica(mock.clone(), "w-0", true).sweep_lease_jobs().await?;

    assert!(sweep.swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn orphan_reclaim_can_be_turned_off() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("w-0"));
    seed_job(
        &mock,
        owned_by(
            without_deadline(lease_job("j", "l1", ORPHAN_RECLAIM_GRACE_SECONDS + 1, 0)?)?,
            "w-2",
        ),
    );
    let provider = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default")
            .with_worker_identity("w-0")
            .with_orphan_reclaim_grace_seconds(0),
    );

    let sweep = provider.sweep_lease_jobs().await?;

    assert!(sweep.swept.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_no_deadline_claimed_warm_pod_of_a_removed_replica_is_reclaimed_once_absent_for_the_grace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("w-0"));
    mock.seed_pod(worker_pod("w-1"));
    mock.seed_pod(aged(
        claimed_warm_pod_due("wp-gone", "l1", "w-2", None),
        ORPHAN_RECLAIM_GRACE_SECONDS + 1,
    )?);
    mock.seed_pod(aged(
        claimed_warm_pod_due("wp-young", "l2", "w-2", None),
        60 - ORPHAN_RECLAIM_GRACE_SECONDS,
    )?);
    mock.seed_pod(aged(
        claimed_warm_pod_due("wp-alive", "l3", "w-1", None),
        ORPHAN_RECLAIM_GRACE_SECONDS + 1,
    )?);
    mock.seed_pod(aged(
        claimed_warm_pod_due("wp-mine", "l4", "w-0", None),
        ORPHAN_RECLAIM_GRACE_SECONDS + 1,
    )?);
    let provider = replica(mock.clone(), "w-0", true);

    for secs in sweep_times(ORPHAN_RECLAIM_GRACE_SECONDS - 1) {
        clock_at(&mock, secs)?;
        assert!(provider.sweep_claimed_warm_pods().await?.is_empty());
    }
    clock_at(&mock, ORPHAN_RECLAIM_GRACE_SECONDS)?;
    let swept = provider.sweep_claimed_warm_pods().await?;

    assert_eq!(swept, vec!["wp-gone".to_string()]);
    let deleted = deleted_names(&mock);
    assert!(deleted.contains(&"wp-gone-restore".to_string()));
    for kept in ["wp-young", "wp-alive", "wp-mine"] {
        assert!(!deleted.contains(&kept.to_string()), "{kept} deleted");
    }
    Ok(())
}

#[tokio::test]
async fn unlabeled_claimed_warm_pods_are_reclaimed_only_when_no_replica_adopts_them() -> TestResult {
    let legacy = || -> FixtureResult<Pod> {
        aged(
            without_owner(claimed_warm_pod_due("wp-legacy", "l1", "x", None))?,
            ORPHAN_RECLAIM_GRACE_SECONDS + 1,
        )
    };

    let adopting = MockK8sOps::new(vec![]);
    adopting.seed_pod(legacy()?);
    let swept = replica(adopting.clone(), "w-0", true).sweep_claimed_warm_pods().await?;
    assert!(swept.is_empty());

    let not_adopting = MockK8sOps::new(vec![]);
    not_adopting.seed_pod(legacy()?);
    let swept = replica(not_adopting.clone(), "w-0", false)
        .sweep_claimed_warm_pods()
        .await?;
    assert_eq!(swept, vec!["wp-legacy".to_string()]);
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_sweep_without_an_apiserver_clock_deletes_nothing() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    *mock.list_clock.locked() = None;
    mock.seed_pod(claimed_warm_pod_due("wp-old", "lease-1", "w-0", Some(-10_000)));

    let swept = replica(mock.clone(), "w-0", true).sweep_claimed_warm_pods().await?;

    assert!(swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_sweep_skips_pods_being_deleted_or_idle() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut deleting = claimed_warm_pod_due("wp-deleting", "lease-1", "w-0", Some(-1));
    deleting.metadata.deletion_timestamp = Some(Time(mock_clock()));
    mock.seed_pod(deleting);
    let mut idle = claimed_warm_pod_due("wp-idle", "lease-2", "w-0", Some(-1));
    idle.metadata
        .labels
        .as_mut()
        .ok_or("missing value")?
        .insert(LABEL_WARM_POOL.to_string(), WARM_POOL_IDLE.to_string());
    mock.seed_pod(idle);

    let swept = replica(mock.clone(), "w-0", true).sweep_claimed_warm_pods().await?;

    assert!(swept.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}
