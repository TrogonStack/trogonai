use super::*;

fn receipt_provider(mock: &Arc<MockK8sOps>) -> K8sMicroVmProvider {
    mock.stamp_identity.store(true, Ordering::SeqCst);
    provider(mock.clone()).with_resource_receipts()
}

#[tokio::test]
async fn a_receipt_scoped_launch_hands_every_created_resource_to_the_job() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = receipt_provider(&mock);

    let handle = p.launch(sample_spec_with_clone(EgressPolicy::DenyAll)?).await?;

    let owners = mock.resource_owners();
    assert!(!owners.is_empty());
    for (resource, owner) in &owners {
        assert_eq!(owner.kind(), K8sResourceKind::Job);
        assert_eq!(owner.name(), handle.id());
        assert_ne!(resource.kind(), K8sResourceKind::Job);
    }
    assert!(
        mock.secret_owners().is_empty(),
        "ownership goes through the UID-fenced path, not the name-keyed one"
    );
    Ok(())
}

#[tokio::test]
async fn a_receipt_scoped_launch_fails_when_the_job_identity_is_not_confirmed() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone()).with_resource_receipts();

    let result = p.launch(sample_spec(EgressPolicy::DenyAll)?).await;

    assert!(result.is_err(), "the apiserver confirmed no namespace or uid");
    Ok(())
}

#[tokio::test]
async fn a_receipt_scoped_warm_pod_owns_the_policy_applied_after_its_claim() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = receipt_provider(&mock);
    let pod_name = p
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;

    let policy = p
        .apply_network_policy_to_claimed_warm_pod(&pod_name, "lease-a", &NetworkPolicy::DenyAll)
        .await?;

    assert!(policy.is_some());
    let owners = mock.resource_owners();
    let (resource, owner) = owners.last().ok_or("no ownership was recorded")?;
    assert_eq!(resource.kind(), K8sResourceKind::NetworkPolicy);
    assert_eq!((owner.kind(), owner.name()), (K8sResourceKind::Pod, pod_name.as_str()));
    Ok(())
}

#[tokio::test]
async fn a_policy_for_a_pod_this_scope_never_recorded_has_no_owner_to_attach_to() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = receipt_provider(&mock);

    let result = p
        .apply_network_policy_to_claimed_warm_pod("wp-elsewhere", "lease-a", &NetworkPolicy::DenyAll)
        .await;

    assert!(
        matches!(result, Err(K8sProviderError::WarmPolicyWithoutOwner)),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_receipt_scoped_workspace_secret_is_recorded() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = receipt_provider(&mock);

    p.create_workspace_tar_secret("workspace-job", "job", b"archive")
        .await?;

    let receipts = p.resource_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].kind(), K8sResourceKind::Secret);
    Ok(())
}
