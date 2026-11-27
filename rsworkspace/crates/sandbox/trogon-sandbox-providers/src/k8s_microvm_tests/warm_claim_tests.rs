use super::*;

async fn running_idle_pod(mock: &Arc<MockK8sOps>, p: &K8sMicroVmProvider) -> FixtureResult<String> {
    let name = p
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    Ok(name)
}

#[tokio::test]
async fn the_warm_pool_listing_keeps_only_pods_this_provider_manages() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let name = running_idle_pod(&mock, &p).await?;
    let mut stranger = worker_pod("stranger");
    stranger.metadata.labels = Some(BTreeMap::from([(
        "app.kubernetes.io/managed-by".to_string(),
        "another-controller".to_string(),
    )]));
    mock.seed_pod(stranger);
    mock.seed_pod(worker_pod("unlabeled"));

    let listing = p.list_warm_pool_pods().await?;

    let names: Vec<_> = listing
        .pods
        .iter()
        .filter_map(|pod| pod.metadata.name.clone())
        .collect();
    assert_eq!(names, vec![name]);
    Ok(())
}

#[tokio::test]
async fn a_pod_of_the_other_naming_family_stops_the_warm_pool_listing() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut legacy = worker_pod("legacy");
    legacy.metadata.labels = Some(BTreeMap::from([(
        "app.kubernetes.io/managed-by".to_string(),
        K8sResourceNaming::Legacy.managed_by().to_string(),
    )]));
    mock.seed_pod(legacy);

    let result = provider(mock).list_warm_pool_pods().await;

    assert!(
        matches!(result, Err(K8sProviderError::ConflictingResourceNaming { .. })),
        "a legacy-named pod must stop the listing"
    );
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_carries_the_configured_image_pull_secrets() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut config = K8sMicroVmConfig::new("default");
    config.image_pull_secrets = vec!["registry-credentials".to_string()];
    let p = K8sMicroVmProvider::new(Arc::clone(&mock) as Arc<dyn K8sOps>, config);

    let name = p
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;

    let pull_secrets = mock
        .pod_named(&name)?
        .spec
        .ok_or("missing value")?
        .image_pull_secrets
        .ok_or("missing value")?;
    assert_eq!(
        pull_secrets.into_iter().map(|secret| secret.name).collect::<Vec<_>>(),
        vec!["registry-credentials".to_string()]
    );
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_owns_its_restore_secret_unless_the_cluster_refuses_or_gives_no_uid() -> TestResult {
    let owned = MockK8sOps::new(vec![]);
    let name = provider(owned.clone())
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    assert_eq!(
        owned
            .secret_owners()
            .into_iter()
            .map(|(secret, _)| secret)
            .collect::<Vec<_>>(),
        vec!["restore-1".to_string()]
    );
    assert_eq!(owned.pod_named(&name)?.metadata.name.as_deref(), Some(name.as_str()));

    let refused = MockK8sOps::new(vec![]);
    refused.owner_errors.store(true, Ordering::SeqCst);
    provider(refused.clone())
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    assert!(refused.secret_owners().is_empty());

    let uidless = MockK8sOps::new(vec![]);
    uidless.uidless.store(true, Ordering::SeqCst);
    provider(uidless.clone())
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    assert!(uidless.secret_owners().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claim_passes_over_pods_it_cannot_fence_or_win() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    running_idle_pod(&mock, &p).await?;
    {
        let mut pods = mock.created_pods.locked();
        let running = pods[0].clone();
        let mut starting = running.clone();
        starting.status = None;
        let mut nameless = running.clone();
        nameless.metadata.name = None;
        let mut unfenced = running;
        unfenced.metadata.name = Some("unfenced".to_string());
        unfenced.metadata.resource_version = None;
        pods.splice(0..0, [starting, nameless, unfenced]);
    }

    mock.claim_conflicts.store(true, Ordering::SeqCst);
    let conflicted = p
        .claim_warm_pod("sha256:abc", "lease-a", "tenant-a", Duration::ZERO)
        .await?;
    mock.claim_conflicts.store(false, Ordering::SeqCst);
    mock.claim_errors.store(true, Ordering::SeqCst);
    let failed = p
        .claim_warm_pod("sha256:abc", "lease-a", "tenant-a", Duration::ZERO)
        .await?;

    assert!(conflicted.is_none());
    assert!(failed.is_none());
    Ok(())
}

#[tokio::test]
async fn a_receipt_scoped_claim_skips_a_pod_whose_identity_was_never_confirmed() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.uidless.store(true, Ordering::SeqCst);
    running_idle_pod(&mock, &provider(mock.clone())).await?;

    let claimed = provider(mock.clone())
        .with_resource_receipts()
        .claim_warm_pod("sha256:abc", "lease-a", "tenant-a", Duration::ZERO)
        .await?;

    assert!(claimed.is_none());
    Ok(())
}
