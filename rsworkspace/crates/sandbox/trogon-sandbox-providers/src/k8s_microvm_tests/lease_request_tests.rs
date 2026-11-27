use super::*;

#[tokio::test]
async fn a_missing_lease_request_loads_as_none() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    assert_eq!(p.load_lease_request("rs-job").await?, None);
    Ok(())
}

#[tokio::test]
async fn a_stored_lease_request_loads_back_and_is_replaced_in_place() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    for body in [b"one".to_vec(), b"two".to_vec()] {
        let mut data = BTreeMap::new();
        data.insert(LEASE_REQUEST_KEY.to_string(), k8s_openapi::ByteString(body));
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(lease_request_secret_name("rs-job")),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };
        mock.put_secret("ns", secret).await?;
    }
    assert_eq!(p.load_lease_request("rs-job").await?, Some(b"two".to_vec()));
    assert_eq!(mock.created_secrets.locked().len(), 1);
    Ok(())
}

#[tokio::test]
async fn storing_a_request_for_a_job_that_does_not_exist_fails() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let result = p
        .store_lease_request(LeaseOwnerKind::Job, "rs-job", "lease", b"{}")
        .await;
    assert!(
        matches!(result, Err(K8sProviderError::LeaseOwnerNotFound { .. })),
        "{result:?}"
    );
    assert!(mock.created_secrets.locked().is_empty());
    Ok(())
}

#[tokio::test]
async fn waiting_for_a_job_that_is_already_gone_returns_at_once() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    p.wait_for_job_gone("rs-job", std::time::Duration::from_secs(1)).await?;
    Ok(())
}
