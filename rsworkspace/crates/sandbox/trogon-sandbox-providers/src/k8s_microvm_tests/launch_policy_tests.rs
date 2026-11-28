use super::*;

#[tokio::test]
async fn launch_sets_the_runtime_default_seccomp_profile() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let profile = pod_spec(&jobs[0])?
        .security_context
        .as_ref()
        .and_then(|sc| sc.seccomp_profile.as_ref())
        .map(|p| p.type_.as_str());
    assert_eq!(profile, Some(SECCOMP_PROFILE_TYPE));
    Ok(())
}

#[tokio::test]
async fn warm_pod_sets_the_runtime_default_seccomp_profile() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;

    let pods = mock.created_pods();
    let profile = pods[0]
        .spec
        .as_ref()
        .and_then(|s| s.security_context.as_ref())
        .and_then(|sc| sc.seccomp_profile.as_ref())
        .map(|p| p.type_.as_str());
    assert_eq!(profile, Some(SECCOMP_PROFILE_TYPE));
    Ok(())
}

#[tokio::test]
async fn launch_job_name_is_deterministic_for_same_inputs() -> TestResult {
    let mock1 = MockK8sOps::new(vec![MockResponse::Ok]);
    let mock2 = MockK8sOps::new(vec![MockResponse::Ok]);

    let spec = sample_spec(EgressPolicy::DenyAll)?;
    provider(mock1.clone()).launch(spec.clone()).await?;
    provider(mock2.clone()).launch(spec).await?;

    let name1 = mock1.created_jobs()[0].metadata.name.clone();
    let name2 = mock2.created_jobs()[0].metadata.name.clone();
    assert_eq!(name1, name2);
    Ok(())
}

#[tokio::test]
async fn launch_job_name_is_valid_rfc1123() -> TestResult {
    let name = job_name("MyOrg", "my-repo", &CommitSha::new("a".repeat(40))?);
    assert!(name.len() <= 63);
    assert!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    );
    assert!(!name.starts_with('-'));
    assert!(!name.ends_with('-'));
    Ok(())
}

#[tokio::test]
async fn launch_with_deny_all_egress_adds_label() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let labels = jobs[0].metadata.labels.as_ref().ok_or("missing value")?;
    assert_eq!(
        labels
            .get(K8sResourceNaming::Sandbox.egress_label())
            .map(|s| s.as_str()),
        Some("deny-all")
    );
    Ok(())
}

#[tokio::test]
async fn launch_with_network_policy_deny_all_creates_network_policy_object() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::AllowHosts(vec![]))?;
    spec.network_policy = Some(NetworkPolicy::DenyAll);

    let p = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("staging").with_runtime_class("kata-fc"),
    );
    p.launch(spec).await?;

    let policies = mock.created_network_policies();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].metadata.namespace.as_deref(), Some("staging"));
    let spec = policies[0].spec.as_ref().ok_or("missing value")?;
    assert_eq!(spec.policy_types, Some(vec!["Egress".to_string()]));
    assert_eq!(spec.egress, Some(vec![]));
    Ok(())
}

#[tokio::test]
async fn launch_with_network_policy_allow_all_creates_zero_network_policies() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::AllowHosts(vec![]))?;
    spec.network_policy = Some(NetworkPolicy::AllowAll);

    provider(mock.clone()).launch(spec).await?;

    assert_eq!(mock.created_network_policies().len(), 0);
    Ok(())
}

#[tokio::test]
async fn cancel_with_network_policy_deletes_network_policy() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok, MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::AllowHosts(vec![]))?;
    spec.network_policy = Some(NetworkPolicy::DenyAll);

    let p = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_runtime_class("kata-fc"),
    );
    let handle = p.launch(spec).await?;
    handle.cancel().await?;

    let calls = mock.delete_calls();
    assert_eq!(calls.len(), 2, "expect delete_job + delete_network_policy");
    assert!(calls.iter().any(|(_, name)| name.ends_with("-netpol")));
    Ok(())
}

#[tokio::test]
async fn status_running_maps_to_running() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let (tx, rx) = watch::channel(SandboxStatus::Running);
    drop(tx);
    let handle = K8sMicroVmHandle {
        name: "rs-acme-widget-abc12345".to_string(),
        namespace: "default".to_string(),
        ops: Arc::clone(&mock) as Arc<dyn K8sOps>,
        cancelled: Arc::new(AtomicBool::new(false)),
        clone_secret_name: None,
        extra_secret_names: vec![],
        network_policy_name: None,
        config_map_name: None,
        status_rx: rx,
        _watcher_handle: Arc::new(tokio::spawn(async {})),
    };

    assert_eq!(handle.status().await, SandboxStatus::Running);
    Ok(())
}

#[tokio::test]
async fn status_succeeded_maps_to_completed() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let (tx, rx) = watch::channel(SandboxStatus::Completed);
    drop(tx);
    let handle = K8sMicroVmHandle {
        name: "rs-acme-widget-abc12345".to_string(),
        namespace: "default".to_string(),
        ops: Arc::clone(&mock) as Arc<dyn K8sOps>,
        cancelled: Arc::new(AtomicBool::new(false)),
        clone_secret_name: None,
        extra_secret_names: vec![],
        network_policy_name: None,
        config_map_name: None,
        status_rx: rx,
        _watcher_handle: Arc::new(tokio::spawn(async {})),
    };

    assert_eq!(handle.status().await, SandboxStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn status_failed_maps_to_failed_with_reason() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let (tx, rx) = watch::channel(SandboxStatus::Failed {
        error: "OOMKilled".to_string(),
    });
    drop(tx);
    let handle = K8sMicroVmHandle {
        name: "rs-acme-widget-abc12345".to_string(),
        namespace: "default".to_string(),
        ops: Arc::clone(&mock) as Arc<dyn K8sOps>,
        cancelled: Arc::new(AtomicBool::new(false)),
        clone_secret_name: None,
        extra_secret_names: vec![],
        network_policy_name: None,
        config_map_name: None,
        status_rx: rx,
        _watcher_handle: Arc::new(tokio::spawn(async {})),
    };

    let status = handle.status().await;
    assert!(
        matches!(&status, SandboxStatus::Failed { error } if error.contains("OOMKilled")),
        "{status:?}"
    );
    Ok(())
}

#[tokio::test]
async fn cancel_sets_status_to_cancelled() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let (tx, rx) = watch::channel(SandboxStatus::Running);
    drop(tx);
    let handle = K8sMicroVmHandle {
        name: "rs-acme-widget-abc12345".to_string(),
        namespace: "default".to_string(),
        ops: Arc::clone(&mock) as Arc<dyn K8sOps>,
        cancelled: Arc::new(AtomicBool::new(false)),
        clone_secret_name: None,
        extra_secret_names: vec![],
        network_policy_name: None,
        config_map_name: None,
        status_rx: rx,
        _watcher_handle: Arc::new(tokio::spawn(async {})),
    };

    handle.cancel().await?;
    assert_eq!(handle.status().await, SandboxStatus::Cancelled);
    Ok(())
}

#[tokio::test]
async fn cancel_calls_delete_job_with_correct_namespace_and_name() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok]);
    let p = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("staging").with_runtime_class("kata-fc"),
    );

    let handle = p.launch(sample_spec(EgressPolicy::DenyAll)?).await?;
    handle.cancel().await?;

    let calls = mock.delete_calls();
    assert_eq!(calls.len(), 2, "expect delete_job + delete_network_policy");
    assert_eq!(calls[0].0, "staging");
    assert!(calls[0].1.starts_with("rs-"));
    Ok(())
}

#[tokio::test]
async fn cancel_treats_not_found_as_success() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::NotFound]);
    let (tx, rx) = watch::channel(SandboxStatus::Running);
    drop(tx);
    let handle = K8sMicroVmHandle {
        name: "rs-acme-widget-gone1234".to_string(),
        namespace: "default".to_string(),
        ops: Arc::clone(&mock) as Arc<dyn K8sOps>,
        cancelled: Arc::new(AtomicBool::new(false)),
        clone_secret_name: None,
        extra_secret_names: vec![],
        network_policy_name: None,
        config_map_name: None,
        status_rx: rx,
        _watcher_handle: Arc::new(tokio::spawn(async {})),
    };

    assert!(handle.cancel().await.is_ok());
    Ok(())
}

#[tokio::test]
async fn launch_with_clone_adds_init_container() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec_with_clone(EgressPolicy::DenyAll)?)
        .await?;

    let secrets = mock.created_secrets();
    assert_eq!(secrets.len(), 1, "expected Secret to be created");

    let jobs = mock.created_jobs();
    assert_eq!(jobs.len(), 1);

    let init_containers = pod_spec(&jobs[0])?.init_containers.as_ref().ok_or("missing value")?;
    assert_eq!(init_containers.len(), 1);
    assert_eq!(init_containers[0].name, "git-clone");
    assert_eq!(init_containers[0].image.as_deref(), Some(GIT_INIT_IMAGE));

    let mounts = init_containers[0].volume_mounts.as_ref().ok_or("missing value")?;
    let paths: Vec<&str> = mounts.iter().map(|m| m.mount_path.as_str()).collect();
    assert!(paths.contains(&WORKSPACE_PATH));
    Ok(())
}

#[tokio::test]
async fn launch_without_clone_has_no_init_container() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    assert_eq!(jobs.len(), 1, "only Job should be created");
    let init_containers = pod_spec(&jobs[0])?.init_containers.as_ref();
    assert!(
        init_containers.map(|v| v.is_empty()).unwrap_or(true),
        "initContainers should be absent when clone is None"
    );
    let secrets = mock.created_secrets();
    assert!(secrets.is_empty(), "no secret should be created");
    Ok(())
}

#[tokio::test]
async fn launch_with_clone_applies_secret_first() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec_with_clone(EgressPolicy::DenyAll)?)
        .await?;

    assert_eq!(mock.created_secrets().len(), 1);
    assert_eq!(mock.created_jobs().len(), 1);
    let secret = &mock.created_secrets()[0];
    assert!(
        secret
            .metadata
            .name
            .as_deref()
            .ok_or("missing value")?
            .ends_with("-clone-creds")
    );
    Ok(())
}

#[tokio::test]
async fn cancel_with_clone_deletes_secret() -> TestResult {
    let mock = MockK8sOps::new(vec![
        MockResponse::Ok,
        MockResponse::Ok,
        MockResponse::Ok,
        MockResponse::Ok,
    ]);
    let p = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_runtime_class("kata-fc"),
    );
    let handle = p.launch(sample_spec_with_clone(EgressPolicy::DenyAll)?).await?;

    handle.cancel().await?;

    let calls = mock.delete_calls();
    assert_eq!(
        calls.len(),
        3,
        "expect delete_job + delete_secret + delete_network_policy"
    );
    Ok(())
}

#[tokio::test]
async fn launch_with_options_applies_sandbox_labels_and_restore_init() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch_with_options(
            sample_spec(EgressPolicy::DenyAll)?,
            K8sLaunchOptions {
                lease_id: Some("lease-abc".to_string()),
                tenant_id: Some("tenant-xyz".to_string()),
                digest_label: Some("sha256:deadbeef".to_string()),
                restore_secret_name: Some("job-restore".to_string()),
                ..Default::default()
            },
        )
        .await?;

    let jobs = mock.created_jobs();
    let job = &jobs[0];
    let labels = job.metadata.labels.as_ref().ok_or("missing value")?;
    assert_eq!(labels.get(LABEL_LEASE_ID).map(String::as_str), Some("lease-abc"));
    assert_eq!(labels.get(LABEL_TENANT_ID).map(String::as_str), Some("tenant-xyz"));
    assert_eq!(labels.get(LABEL_DIGEST).map(String::as_str), Some("deadbeef"));

    let init = pod_spec(job)?.init_containers.as_ref().ok_or("missing value")?;
    assert!(init.iter().any(|c| c.name == "restore-snapshot"));
    Ok(())
}

#[tokio::test]
async fn push_network_policy_patches_config_map_with_new_policy_json() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let job_name = "rs-acme-widget-abc12345";
    let configmap_name = crate::k8s_network_policy::egress_proxy_configmap_name(job_name);
    let new_policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };

    p.push_network_policy(
        job_name,
        Some(&configmap_name),
        None,
        &new_policy,
        &pod_selector_labels_for(job_name),
    )
    .await?;

    let patches = mock.patched_config_maps();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].0, "default");
    assert_eq!(patches[0].1, configmap_name);
    let policy_json = patches[0]
        .2
        .get(crate::k8s_network_policy::EGRESS_PROXY_CONFIGMAP_KEY)
        .ok_or("missing value")?;
    let round_tripped: NetworkPolicy = serde_json::from_str(policy_json)?;
    assert_eq!(round_tripped, new_policy);

    assert_eq!(mock.created_jobs().len(), 0, "must not touch the Job");
    assert_eq!(mock.created_pods().len(), 0, "must not touch any Pod");
    Ok(())
}

#[tokio::test]
async fn push_network_policy_patches_network_policy_with_manifest_spec() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let job_name = "rs-acme-widget-abc12345";
    let network_policy_name = crate::k8s_network_policy::network_policy_name(job_name);
    let new_policy = NetworkPolicy::DenyAll;
    let labels = pod_selector_labels_for(job_name);

    p.push_network_policy(job_name, None, Some(&network_policy_name), &new_policy, &labels)
        .await?;

    let patches = mock.patched_network_policies();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].0, "default");
    assert_eq!(patches[0].1, network_policy_name);

    let expected_manifest =
        build_network_policy_manifest(&labels, &new_policy, "default", job_name).ok_or("missing value")?;
    assert_eq!(patches[0].2, expected_manifest.spec.ok_or("missing value")?);

    assert_eq!(mock.delete_calls().len(), 0, "must patch, not delete");
    assert_eq!(mock.created_jobs().len(), 0, "must not touch the Job");
    Ok(())
}

#[tokio::test]
async fn push_network_policy_allow_all_transition_deletes_rather_than_patches() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let job_name = "rs-acme-widget-abc12345";
    let network_policy_name = crate::k8s_network_policy::network_policy_name(job_name);
    let labels = pod_selector_labels_for(job_name);

    p.push_network_policy(
        job_name,
        None,
        Some(&network_policy_name),
        &NetworkPolicy::AllowAll,
        &labels,
    )
    .await?;

    assert_eq!(
        mock.patched_network_policies().len(),
        0,
        "allow-all transition must not patch"
    );
    let deletes = mock.delete_calls();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0], ("default".to_string(), network_policy_name));
    Ok(())
}

#[tokio::test]
async fn push_network_policy_creates_the_object_for_an_allow_all_lease() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let job_name = "rs-acme-widget-abc12345";
    let labels = pod_selector_labels_for(job_name);

    let in_force = p
        .push_network_policy(job_name, None, None, &NetworkPolicy::DenyAll, &labels)
        .await?;

    let expected = crate::k8s_network_policy::network_policy_name(job_name);
    assert_eq!(in_force.as_deref(), Some(expected.as_str()));
    assert_eq!(mock.created_network_policies().len(), 1);
    assert_eq!(mock.patched_network_policies().len(), 0);
    Ok(())
}

#[tokio::test]
async fn push_network_policy_reports_none_after_the_allow_all_delete() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let job_name = "rs-acme-widget-abc12345";
    let name = crate::k8s_network_policy::network_policy_name(job_name);
    let labels = pod_selector_labels_for(job_name);

    let in_force = p
        .push_network_policy(job_name, None, Some(&name), &NetworkPolicy::AllowAll, &labels)
        .await?;

    assert_eq!(in_force, None);
    Ok(())
}

#[tokio::test]
async fn push_network_policy_has_no_job_or_pod_side_effects() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    let job_name = "rs-acme-widget-abc12345";
    let configmap_name = crate::k8s_network_policy::egress_proxy_configmap_name(job_name);
    let network_policy_name = crate::k8s_network_policy::network_policy_name(job_name);
    let labels = pod_selector_labels_for(job_name);

    p.push_network_policy(
        job_name,
        Some(&configmap_name),
        Some(&network_policy_name),
        &NetworkPolicy::DenyAll,
        &labels,
    )
    .await?;

    assert_eq!(mock.created_jobs().len(), 0);
    assert_eq!(mock.created_pods().len(), 0);
    assert_eq!(mock.created_secrets().len(), 0);
    assert_eq!(mock.created_network_policies().len(), 0);
    assert_eq!(mock.created_config_maps().len(), 0);
    assert_eq!(mock.delete_calls().len(), 0);
    Ok(())
}

#[tokio::test]
async fn handle_exposes_config_map_name_and_network_policy_name_accessors() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let (tx, rx) = watch::channel(SandboxStatus::Running);
    drop(tx);
    let handle = K8sMicroVmHandle {
        name: "rs-acme-widget-abc12345".to_string(),
        namespace: "default".to_string(),
        ops: Arc::clone(&mock) as Arc<dyn K8sOps>,
        cancelled: Arc::new(AtomicBool::new(false)),
        clone_secret_name: None,
        extra_secret_names: vec![],
        network_policy_name: Some("rs-acme-widget-abc12345-netpol".to_string()),
        config_map_name: Some("rs-acme-widget-abc12345-egress-policy".to_string()),
        status_rx: rx,
        _watcher_handle: Arc::new(tokio::spawn(async {})),
    };

    assert_eq!(handle.network_policy_name(), Some("rs-acme-widget-abc12345-netpol"));
    assert_eq!(handle.config_map_name(), Some("rs-acme-widget-abc12345-egress-policy"));
    Ok(())
}
