use super::*;

#[tokio::test]
async fn cancelling_a_lease_deletes_its_job_secrets_and_network_policy() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let handle = provider(mock.clone())
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
    let job_name = handle.id().to_string();
    let policy = handle.network_policy_name().ok_or("no network policy")?.to_string();

    assert_ne!(handle.status().await, SandboxStatus::Cancelled);
    handle.cancel().await?;

    let deleted = deleted_names(&mock);
    for name in [
        job_name.clone(),
        clone_secret_name(&job_name),
        "job-restore".to_string(),
        "job-s3-volume".to_string(),
        policy,
    ] {
        assert!(deleted.contains(&name), "{name} not deleted: {deleted:?}");
    }
    assert_eq!(handle.status().await, SandboxStatus::Cancelled);
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_surfaces_from_cancel_but_still_marks_the_lease_cancelled() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let handle = provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;
    mock.failing_deletes.locked().insert(handle.id().to_string());

    let result = handle.cancel().await;

    assert!(result.is_err());
    assert_eq!(handle.status().await, SandboxStatus::Cancelled);
    Ok(())
}
