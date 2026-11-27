use super::*;

fn nameless(mut job: Job) -> Job {
    job.metadata.name = None;
    job
}

#[tokio::test]
async fn tearing_down_reports_the_first_object_the_cluster_refuses_to_delete() -> TestResult {
    for refused in ["job", "clone", "extra", "policy"] {
        let mock = MockK8sOps::new(vec![]);
        mock.failing_deletes.locked().insert(refused.to_string());

        let result = provider(mock.clone())
            .tear_down("job", Some("clone"), &["extra"], Some("policy"), None)
            .await;

        assert!(result.is_err(), "{refused} refused but the tear down succeeded");
    }
    Ok(())
}

#[tokio::test]
async fn tearing_down_only_the_job_leaves_every_optional_object_alone() -> TestResult {
    let mock = MockK8sOps::new(vec![]);

    provider(mock.clone()).tear_down("job", None, &[], None, None).await?;

    assert_eq!(deleted_names(&mock), vec!["job".to_string()]);
    Ok(())
}

#[tokio::test]
async fn a_lease_job_that_cannot_be_torn_down_is_kept_for_the_next_sweep() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, lease_job("job-old", "lease-1", 301, 300)?);
    seed_job(
        &mock,
        owned_by(
            lease_job("job-dead", "lease-2", 300 + FOREIGN_LEASE_GRACE_SECONDS, 300)?,
            "w-1",
        ),
    );
    mock.failing_deletes.locked().insert("job-old".to_string());
    mock.failing_deletes.locked().insert("job-dead".to_string());

    let sweep = replica(mock.clone(), "w-0", true).sweep_lease_jobs().await?;

    assert_eq!(sweep, LeaseJobSweep::default());
    Ok(())
}

#[tokio::test]
async fn a_sweep_skips_a_job_that_has_no_name() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, nameless(lease_job("ignored", "lease-1", 301, 300)?));

    let sweep = provider(mock.clone()).sweep_lease_jobs().await?;

    assert_eq!(sweep, LeaseJobSweep::default());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claimed_warm_pod_that_cannot_be_deleted_is_kept_for_the_next_sweep() -> TestResult {
    for refused in ["wp-old", "wp-old-netpol"] {
        let mock = MockK8sOps::new(vec![]);
        mock.seed_pod(claimed_warm_pod_due("wp-old", "lease-1", "w-0", Some(-1)));
        mock.failing_deletes.locked().insert(if refused == "wp-old" {
            refused.to_string()
        } else {
            crate::k8s_network_policy::network_policy_name("wp-old")
        });

        let swept = replica(mock.clone(), "w-0", true).sweep_claimed_warm_pods().await?;

        assert!(swept.is_empty(), "{refused}");
    }
    Ok(())
}

#[tokio::test]
async fn the_claimed_warm_pod_sweeps_skip_nameless_idle_and_foreign_pods() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut nameless_pod = claimed_warm_pod_due("wp-x", "lease-1", "w-0", Some(-1));
    nameless_pod.metadata.name = None;
    mock.seed_pod(nameless_pod);
    let mut idle = claimed_warm_pod_due("wp-idle", "lease-2", "w-0", Some(-1));
    idle.metadata
        .labels
        .as_mut()
        .ok_or("missing value")?
        .insert(LABEL_WARM_POOL.to_string(), WARM_POOL_IDLE.to_string());
    mock.seed_pod(idle);
    mock.seed_pod(claimed_warm_pod_due("wp-theirs", "lease-3", "w-9", Some(60)));
    mock.seed_pod(worker_pod("bare"));
    let p = replica(mock.clone(), "w-0", true);

    assert!(p.sweep_claimed_warm_pods().await?.is_empty());
    assert!(p.claimed_warm_pods().await?.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn is_orphaned_needs_an_identity_a_creation_time_and_a_grace() -> TestResult {
    let labels = BTreeMap::from([(LABEL_WORKER_IDENTITY.to_string(), "w-2".to_string())]);
    let old = Time(mock_clock().checked_sub(k8s_openapi::jiff::SignedDuration::from_secs(10_000))?);
    let now = mock_clock();
    let mut live = None;

    let anonymous = provider(MockK8sOps::new(vec![]));
    assert!(!anonymous.is_orphaned(&labels, Some(&old), now, &mut live).await?);

    let named = replica(MockK8sOps::new(vec![]), "w-0", true);
    assert!(!named.is_orphaned(&labels, None, now, &mut live).await?);

    let off = K8sMicroVmProvider::new(
        MockK8sOps::new(vec![]) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default")
            .with_worker_identity("w-0")
            .with_orphan_reclaim_grace_seconds(0),
    );
    assert!(!off.is_orphaned(&labels, Some(&old), now, &mut live).await?);

    let mine = BTreeMap::from([(LABEL_WORKER_IDENTITY.to_string(), "w-0".to_string())]);
    assert!(!named.is_orphaned(&mine, Some(&old), now, &mut live).await?);
    Ok(())
}

#[tokio::test]
async fn a_long_gap_between_sightings_restarts_the_absence_count() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(worker_pod("w-0"));
    let p = replica(mock.clone(), "w-0", true);
    let labels = BTreeMap::from([(LABEL_WORKER_IDENTITY.to_string(), "w-2".to_string())]);
    let old = Time(mock_clock().checked_sub(k8s_openapi::jiff::SignedDuration::from_secs(100_000))?);
    let later = mock_clock().checked_add(k8s_openapi::jiff::SignedDuration::from_secs(
        10 * FOREIGN_LEASE_GRACE_SECONDS,
    ))?;

    assert!(!p.is_orphaned(&labels, Some(&old), mock_clock(), &mut None).await?);
    assert!(
        !p.is_orphaned(&labels, Some(&old), later, &mut None).await?,
        "an unwatched gap proves nothing, so the count starts over"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_for_a_pod_skips_nameless_pods_and_times_out_while_it_starts() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut anonymous = job_pod("ignored", "Pending", waiting("ContainerCreating"));
    anonymous.metadata.name = None;
    mock.seed_pod(anonymous);
    mock.seed_pod(job_pod("job-1-abcde", "Pending", waiting("ContainerCreating")));

    let result = provider(mock).wait_for_job_pod("job-1", Duration::from_millis(1)).await;

    assert!(
        matches!(result, Err(K8sProviderError::PodStartTimeout { .. })),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_for_a_job_that_stays_present_times_out() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, lease_job("job-1", "lease-1", 5, 300)?);

    let result = provider(mock)
        .wait_for_job_gone("job-1", Duration::from_millis(1))
        .await;

    assert!(
        matches!(result, Err(K8sProviderError::JobDeletionTimeout { .. })),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn recovering_and_sweeping_claimed_warm_pods_refuse_the_other_naming_family() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut legacy = claimed_warm_pod_due("wp-legacy", "lease-1", "w-0", Some(-1));
    legacy.metadata.labels = Some(BTreeMap::from([(
        "app.kubernetes.io/managed-by".to_string(),
        K8sResourceNaming::Legacy.managed_by().to_string(),
    )]));
    mock.seed_pod(legacy);
    let p = replica(mock.clone(), "w-0", true);

    let recovered = p.claimed_warm_pods().await;
    let swept = p.sweep_claimed_warm_pods().await;

    assert!(matches!(
        recovered,
        Err(K8sProviderError::ConflictingResourceNaming { .. })
    ));
    assert!(matches!(swept, Err(K8sProviderError::ConflictingResourceNaming { .. })));
    assert!(mock.delete_calls().is_empty());
    Ok(())
}
