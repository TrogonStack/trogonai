use super::*;

fn owned_job(name: &str, uid: &str) -> FixtureResult<Job> {
    let mut job = lease_job(name, "lease-1", 5, 300)?;
    job.metadata.uid = Some(uid.to_string());
    Ok(job)
}

#[tokio::test]
async fn a_workspace_archive_secret_is_created_in_the_namespace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);

    provider(mock.clone())
        .create_workspace_tar_secret("workspace-job", "job", b"archive")
        .await?;

    let secrets = mock.created_secrets();
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].metadata.name.as_deref(), Some("workspace-job"));
    Ok(())
}

#[tokio::test]
async fn a_lease_request_is_stored_under_the_uid_of_its_job_and_loads_back() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    seed_job(&mock, owned_job("job-x", "job-uid")?);
    let p = provider(mock.clone());

    let name = p
        .store_lease_request(LeaseOwnerKind::Job, "job-x", "lease-1", b"{\"a\":1}")
        .await?;

    assert_eq!(name, lease_request_secret_name("job-x"));
    let stored = mock
        .created_secrets()
        .into_iter()
        .find(|secret| secret.metadata.name.as_deref() == Some(name.as_str()))
        .ok_or("the request secret was not stored")?;
    let owner = stored
        .metadata
        .owner_references
        .ok_or("missing value")?
        .into_iter()
        .next()
        .ok_or("missing value")?;
    assert_eq!(
        (owner.api_version.as_str(), owner.kind.as_str(), owner.uid.as_str()),
        ("batch/v1", "Job", "job-uid")
    );
    assert_eq!(p.load_lease_request("job-x").await?, Some(b"{\"a\":1}".to_vec()));
    Ok(())
}

#[tokio::test]
async fn a_lease_request_of_a_warm_pod_is_owned_by_the_pod() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut pod = worker_pod("wp-1");
    pod.metadata.uid = Some("pod-uid".to_string());
    mock.seed_pod(pod);

    let name = provider(mock.clone())
        .store_lease_request(LeaseOwnerKind::Pod, "wp-1", "lease-1", b"{}")
        .await?;

    let stored = mock
        .created_secrets()
        .into_iter()
        .find(|secret| secret.metadata.name.as_deref() == Some(name.as_str()))
        .ok_or("the request secret was not stored")?;
    let owner = stored
        .metadata
        .owner_references
        .ok_or("missing value")?
        .into_iter()
        .next()
        .ok_or("missing value")?;
    assert_eq!((owner.kind.as_str(), owner.uid.as_str()), ("Pod", "pod-uid"));
    Ok(())
}

#[tokio::test]
async fn storing_a_request_for_a_pod_that_does_not_exist_fails() -> TestResult {
    let result = provider(MockK8sOps::new(vec![]))
        .store_lease_request(LeaseOwnerKind::Pod, "wp-missing", "lease-1", b"{}")
        .await;

    assert!(
        matches!(result, Err(K8sProviderError::LeaseOwnerNotFound { kind: "Pod", .. })),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_named_secret_is_deleted_from_the_namespace() -> TestResult {
    let mock = MockK8sOps::new(vec![]);

    provider(mock.clone()).delete_secret_named("stray").await?;

    assert_eq!(mock.delete_calls(), vec![("default".to_string(), "stray".to_string())]);
    Ok(())
}
