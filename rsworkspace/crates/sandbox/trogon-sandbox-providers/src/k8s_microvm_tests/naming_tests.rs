use super::*;

#[tokio::test]
async fn legacy_resource_names_are_explicit_and_process_metadata_stays_generic() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let provider = K8sMicroVmProvider::new(
        mock.clone(),
        K8sMicroVmConfig::new("default")
            .with_resource_naming(K8sResourceNaming::Legacy)
            .with_service_account("retained-agent-account"),
    );
    provider.launch(sample_spec(EgressPolicy::DenyAll)?).await?;

    let jobs = mock.created_jobs();
    let labels = jobs[0].metadata.labels.as_ref().ok_or("missing value")?;
    assert!(labels.contains_key(K8sResourceNaming::Legacy.job_label()));
    assert!(!labels.contains_key(K8sResourceNaming::Sandbox.job_label()));
    assert_eq!(
        pod_spec(&jobs[0])?.service_account_name.as_deref(),
        Some("retained-agent-account")
    );
    assert_eq!(
        find_env(agent_container(&jobs[0])?, "SANDBOX_OWNER").as_deref(),
        Some("acme")
    );
    assert!(
        agent_container(&jobs[0])?
            .env
            .as_ref()
            .ok_or("missing value")?
            .iter()
            .all(|entry| !entry.name.starts_with("LEGACY_"))
    );

    provider
        .create_workspace_tar_secret("workspace", "job", b"archive")
        .await?;
    let secrets = mock.created_secrets();
    let labels = secrets
        .last()
        .ok_or("missing value")?
        .metadata
        .labels
        .as_ref()
        .ok_or("missing value")?;
    assert_eq!(
        labels.get("app.kubernetes.io/managed-by").map(String::as_str),
        Some(K8sResourceNaming::Legacy.managed_by())
    );
    Ok(())
}

#[tokio::test]
async fn switching_resource_names_refuses_recovery_before_any_cleanup() -> TestResult {
    for naming in [K8sResourceNaming::Sandbox, K8sResourceNaming::Legacy] {
        let mock = MockK8sOps::new(vec![]);
        let mut old_job = lease_job("old-job", "lease", 301, 300)?;
        old_job
            .metadata
            .labels
            .as_mut()
            .ok_or("missing value")?
            .insert(naming.other().job_label().into(), "old-job".into());
        seed_job(&mock, old_job);
        let provider = K8sMicroVmProvider::new(
            mock.clone(),
            K8sMicroVmConfig::new("default").with_resource_naming(naming),
        );
        let result = provider.sweep_lease_jobs().await;
        assert!(
            matches!(result, Err(K8sProviderError::ConflictingResourceNaming { .. })),
            "{result:?}"
        );
        assert!(mock.delete_calls().is_empty());
        assert_eq!(provider.resource_naming(), naming);
    }
    Ok(())
}

#[tokio::test]
async fn foreign_warm_pool_resources_are_never_returned_claimed_or_deleted() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let provider = provider(mock.clone());
    provider
        .create_warm_pod("sha256:abc", "image", "restore", "", 1.0, 64)
        .await?;
    mock.mark_pods_running();
    {
        let mut pods = mock.created_pods.locked();
        let labels = pods[0].metadata.labels.as_mut().ok_or("missing value")?;
        labels.insert("app.kubernetes.io/managed-by".into(), "unrelated-controller".into());
    }
    assert!(provider.list_warm_pool_pods().await?.pods.is_empty());
    assert!(
        provider
            .claim_warm_pod("sha256:abc", "lease", "tenant", Duration::from_secs(30))
            .await?
            .is_none()
    );
    {
        let mut pods = mock.created_pods.locked();
        let labels = pods[0].metadata.labels.as_mut().ok_or("missing value")?;
        labels.insert(LABEL_WARM_POOL.into(), WARM_POOL_CLAIMED.into());
        labels.insert(LABEL_LEASE_ID.into(), "lease".into());
        labels.insert(LABEL_LEASE_DEADLINE.into(), "0".into());
    }
    assert!(provider.claimed_warm_pods().await?.is_empty());
    assert!(provider.sweep_claimed_warm_pods().await?.is_empty());
    assert!(mock.delete_calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn claimed_warm_pods_are_reported_with_their_lease_and_secret() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let pod_with = |name: &str, state: &str, lease: Option<&str>| {
        let mut labels = BTreeMap::from([(LABEL_WARM_POOL.to_string(), state.to_string())]);
        labels.insert(
            "app.kubernetes.io/managed-by".into(),
            K8sResourceNaming::Sandbox.managed_by().into(),
        );
        if let Some(lease) = lease {
            labels.insert(LABEL_LEASE_ID.to_string(), lease.to_string());
        }
        Pod {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(PodSpec {
                volumes: Some(vec![Volume {
                    name: RESTORE_VOLUME_NAME.to_string(),
                    secret: Some(SecretVolumeSource {
                        secret_name: Some(format!("{name}-restore")),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    };
    mock.seed_pod(pod_with("warm-claimed", WARM_POOL_CLAIMED, Some("lease-1")));
    mock.seed_pod(pod_with("warm-idle", WARM_POOL_IDLE, None));

    let claimed = provider(mock).claimed_warm_pods().await?;

    assert_eq!(
        claimed,
        vec![RecoveredWarmLease {
            lease_id: "lease-1".to_string(),
            pod_name: "warm-claimed".to_string(),
            restore_secret_name: Some("warm-claimed-restore".to_string()),
        }]
    );
    Ok(())
}
