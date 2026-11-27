use super::*;

#[tokio::test]
async fn waiting_for_a_job_pod_returns_the_pod_name() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(job_pod("rs-sandbox-job-x7k2p", "Running", running()));
    let name = provider(mock)
        .wait_for_job_pod("rs-sandbox-job", std::time::Duration::from_secs(5))
        .await?;
    assert_eq!(name, "rs-sandbox-job-x7k2p");
    Ok(())
}

#[tokio::test]
async fn waiting_for_a_job_pod_fails_fast_when_it_cannot_start() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.seed_pod(job_pod("p", "Pending", waiting("ErrImagePull")));
    let started = std::time::Instant::now();
    let result = provider(mock)
        .wait_for_job_pod("job", std::time::Duration::from_secs(60))
        .await;
    assert!(
        matches!(&result, Err(K8sProviderError::PodFailedToStart { reason, .. }) if reason.contains("ErrImagePull")),
        "{result:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn waiting_for_a_job_pod_times_out() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let result = provider(mock)
        .wait_for_job_pod("job", std::time::Duration::from_millis(10))
        .await;
    assert!(
        matches!(result, Err(K8sProviderError::PodStartTimeout { .. })),
        "{result:?}"
    );
    Ok(())
}
