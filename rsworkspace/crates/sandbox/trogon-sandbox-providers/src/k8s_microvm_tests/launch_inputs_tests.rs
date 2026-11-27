use super::*;
use trogon_sandbox_contract::{EmptyObject, HostMountAccess};

#[tokio::test]
async fn a_host_directory_workspace_is_refused() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.host_mount = Some(HostMount {
        path: "/srv/workspace".into(),
        access: HostMountAccess::ReadWrite,
    });

    let result = provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await;

    assert!(
        matches!(result, Err(K8sProviderError::HostMountUnsupported)),
        "a node path cannot back a pod workspace"
    );
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn every_clone_auth_stores_its_credential_in_the_clone_secret() -> TestResult {
    let auths = [
        (CloneAuth::None(EmptyObject {}), Vec::<u8>::new()),
        (
            CloneAuth::BearerToken {
                token_ref: SecretRef::Literal {
                    value: "bearer-value".to_string(),
                },
            },
            b"bearer-value".to_vec(),
        ),
        (
            CloneAuth::Basic {
                username: "builder".to_string(),
                password_ref: SecretRef::Literal {
                    value: "basic-value".to_string(),
                },
            },
            b"basic-value".to_vec(),
        ),
    ];
    for (auth, expected) in auths {
        let mock = MockK8sOps::new(vec![]);
        let mut spec = sample_spec_with_clone(EgressPolicy::DenyAll)?;
        spec.clone.as_mut().ok_or("missing value")?.auth = auth;

        provider(mock.clone()).launch(spec).await?;

        let secrets = mock.created_secrets();
        let stored: Vec<Vec<u8>> = secrets
            .iter()
            .flat_map(|secret| secret.data.iter().flat_map(|data| data.values().map(|b| b.0.clone())))
            .collect();
        assert_eq!(stored, vec![expected]);
    }
    Ok(())
}

#[tokio::test]
async fn a_nats_backed_clone_credential_needs_a_nats_connection() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut spec = sample_spec_with_clone(EgressPolicy::DenyAll)?;
    spec.clone.as_mut().ok_or("missing value")?.auth = CloneAuth::BearerToken {
        token_ref: SecretRef::NatsKv {
            bucket: "secrets".to_string(),
            key: "token".to_string(),
        },
    };

    let result = provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await;

    assert!(matches!(result, Err(K8sProviderError::Secret(_))), "{:?}", result.err());
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_invalid_allowed_host_is_dropped_and_the_valid_ones_stay() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let hosts = vec!["not a host".to_string(), "registry.example.com".to_string()];

    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::AllowHosts(hosts))?)
        .await?;

    assert_eq!(mock.created_jobs().len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_job_without_a_uid_cannot_own_its_secrets_but_still_launches() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.uidless.store(true, Ordering::SeqCst);

    provider(mock.clone())
        .launch(sample_spec_with_clone(EgressPolicy::DenyAll)?)
        .await?;

    assert_eq!(mock.created_jobs().len(), 1);
    assert!(mock.secret_owners().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_refused_secret_owner_does_not_fail_the_launch() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    mock.owner_errors.store(true, Ordering::SeqCst);

    provider(mock.clone())
        .launch(sample_spec_with_clone(EgressPolicy::DenyAll)?)
        .await?;

    assert_eq!(mock.created_jobs().len(), 1);
    assert!(mock.secret_owners().is_empty());
    Ok(())
}

#[tokio::test]
async fn tightening_an_open_lease_creates_the_network_policy_it_lacked() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let labels = pod_selector_labels_for("job-1");

    let created = p
        .push_network_policy("job-1", None, None, &NetworkPolicy::DenyAll, &labels)
        .await?;
    let open = p
        .push_network_policy("job-2", None, None, &NetworkPolicy::AllowAll, &labels)
        .await?;

    assert_eq!(mock.created_network_policies().len(), 1);
    assert_eq!(
        created,
        mock.created_network_policies()[0].metadata.name.clone(),
        "the caller learns the name of the policy now in force"
    );
    assert_eq!(open, None);
    Ok(())
}
