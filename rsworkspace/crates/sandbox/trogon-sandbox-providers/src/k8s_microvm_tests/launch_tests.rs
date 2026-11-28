use super::*;
use crate::RunAsUserError;

#[tokio::test]
async fn launch_leaves_runtime_class_unset_when_configured_empty() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .with_runtime_class_override("")
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    assert_eq!(pod_spec(&jobs[0])?.runtime_class_name, None);
    Ok(())
}

#[tokio::test]
async fn launch_applies_job_manifest_with_runtime_class() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];

    assert_eq!(pod_spec(job)?.runtime_class_name.as_deref(), Some("kata-fc"));
    assert_eq!(job_spec(job)?.backoff_limit, Some(0));
    assert_eq!(agent_container(job)?.image.as_deref(), Some("sandbox-agent:latest"));
    Ok(())
}

#[tokio::test]
async fn launch_sets_resource_limits_from_spec() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let resources = agent_container(&jobs[0])?.resources.as_ref().ok_or("missing value")?;

    assert_eq!(
        resources
            .limits
            .as_ref()
            .ok_or("missing value")?
            .get("cpu")
            .ok_or("missing value")?
            .0,
        "2"
    );
    assert_eq!(
        resources
            .limits
            .as_ref()
            .ok_or("missing value")?
            .get("memory")
            .ok_or("missing value")?
            .0,
        "1024Mi"
    );
    assert_eq!(
        resources
            .requests
            .as_ref()
            .ok_or("missing value")?
            .get("cpu")
            .ok_or("missing value")?
            .0,
        "2"
    );
    assert_eq!(
        resources
            .requests
            .as_ref()
            .ok_or("missing value")?
            .get("memory")
            .ok_or("missing value")?
            .0,
        "1024Mi"
    );
    Ok(())
}

#[tokio::test]
async fn launch_sets_active_deadline_seconds_from_timeout() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    assert_eq!(job_spec(&jobs[0])?.active_deadline_seconds, Some(300));
    Ok(())
}

#[tokio::test]
async fn launch_without_a_timeout_sets_no_deadline() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.timeout = std::time::Duration::ZERO;
    provider(mock.clone()).launch(spec).await?;

    let jobs = mock.created_jobs();
    assert_eq!(job_spec(&jobs[0])?.active_deadline_seconds, None);
    let container = agent_container(&jobs[0])?;
    assert_eq!(find_env(container, "SANDBOX_TIMEOUT_SECS"), None);
    Ok(())
}

#[tokio::test]
async fn launch_includes_all_required_env_vars() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let container = agent_container(&jobs[0])?;

    assert_eq!(find_env(container, "SANDBOX_OWNER").as_deref(), Some("acme"));
    assert_eq!(find_env(container, "SANDBOX_REPO").as_deref(), Some("widget"));
    assert_eq!(
        find_env(container, "SANDBOX_RESOLVED_SHA").as_deref(),
        Some("a".repeat(40).as_str())
    );
    assert_eq!(
        find_env(container, "SANDBOX_WORKSPACE").as_deref(),
        Some(WORKSPACE_PATH)
    );
    assert_eq!(
        find_env(container, "SANDBOX_EVIDENCE_ROOT").as_deref(),
        Some(EVIDENCE_PATH)
    );
    assert_eq!(find_env(container, "SANDBOX_TIMEOUT_SECS").as_deref(), Some("300"));
    assert_eq!(find_env(container, "SANDBOX_TOOLS").as_deref(), Some("diff,entities"));
    let provider_val = find_env(container, "SANDBOX_PROVIDER").ok_or("missing value")?;
    assert!(!provider_val.is_empty());
    Ok(())
}

#[tokio::test]
async fn launch_passes_the_lease_env_to_the_agent_container() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.env.insert("MY_VAR".into(), "one".into());
    provider(mock.clone()).launch(spec).await?;

    let jobs = mock.created_jobs();
    let container = agent_container(&jobs[0])?;
    assert_eq!(find_env(container, "MY_VAR").as_deref(), Some("one"));
    assert!(find_env(container, "SANDBOX_OWNER").is_some());
    Ok(())
}

#[tokio::test]
async fn launch_omits_base_sha_when_absent() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let container = agent_container(&jobs[0])?;
    assert!(find_env(container, "SANDBOX_BASE_SHA").is_none());
    Ok(())
}

#[tokio::test]
async fn launch_includes_base_sha_when_present() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec_with_base(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let container = agent_container(&jobs[0])?;
    assert_eq!(
        find_env(container, "SANDBOX_BASE_SHA").as_deref(),
        Some("b".repeat(40).as_str())
    );
    Ok(())
}

#[tokio::test]
async fn launch_mounts_workspace_and_evidence_volumes() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let ps = pod_spec(&jobs[0])?;
    let vols = ps.volumes.as_ref().ok_or("missing value")?;
    let vol_names: Vec<&str> = vols.iter().map(|v| v.name.as_str()).collect();
    assert!(vol_names.contains(&"workspace"));
    assert!(vol_names.contains(&"evidence"));

    let mounts = agent_container(&jobs[0])?
        .volume_mounts
        .as_ref()
        .ok_or("missing value")?;
    let mount_paths: Vec<&str> = mounts.iter().map(|m| m.mount_path.as_str()).collect();
    assert!(mount_paths.contains(&WORKSPACE_PATH));
    assert!(mount_paths.contains(&EVIDENCE_PATH));
    Ok(())
}

#[tokio::test]
async fn cold_runtime_tmp_is_bounded_ephemeral_and_preserves_root_hardening() -> TestResult {
    for memory_mib in [1024, 0] {
        let mock = MockK8sOps::new(vec![MockResponse::Ok]);
        let mut spec = sample_spec(EgressPolicy::DenyAll)?;
        spec.memory_mib = memory_mib;
        provider(mock.clone()).launch(spec).await?;
        assert_bounded_runtime_tmp(pod_spec(&mock.created_jobs()[0])?, memory_mib)?;
    }
    Ok(())
}

#[tokio::test]
async fn warm_runtime_tmp_is_bounded_ephemeral_and_preserves_root_hardening() -> TestResult {
    for memory_mib in [256, 0] {
        let mock = MockK8sOps::new(vec![MockResponse::Ok]);
        provider(mock.clone())
            .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, memory_mib)
            .await?;
        assert_bounded_runtime_tmp(mock.created_pods()[0].spec.as_ref().ok_or("missing value")?, memory_mib)?;
    }
    Ok(())
}

#[tokio::test]
async fn launch_with_domain_rules_wires_ca_trust_into_agent_container() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok, MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.network_policy = Some(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::Transform(CredentialTransform {
                match_rules: vec![],
                credential: CredentialRequest {
                    provider: CredentialProvider::Registry,
                    pull_secret_id: PullSecretId::new("test-secret"),
                    scope: CredentialScope::RegistryPull {
                        repository_glob: "*".to_string(),
                    },
                    ttl_seconds: 60,
                    requester: None,
                },
                injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
            }),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    });

    provider(mock.clone()).launch(spec).await?;

    let jobs = mock.created_jobs();
    let ps = pod_spec(&jobs[0])?;
    let vol_names: Vec<&str> = ps
        .volumes
        .as_ref()
        .ok_or("missing value")?
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert!(vol_names.contains(&EGRESS_PROXY_CA_VOLUME_NAME));

    let agent = agent_container(&jobs[0])?;
    let mounts = agent.volume_mounts.as_ref().ok_or("missing value")?;
    assert!(mounts.iter().any(|m| m.name == EGRESS_PROXY_CA_VOLUME_NAME
        && m.mount_path == EGRESS_PROXY_CA_MOUNT_PATH
        && m.read_only == Some(true)));

    let ca_file = format!("{EGRESS_PROXY_CA_MOUNT_PATH}/ca.pem");
    for name in CA_TRUST_ENV_VARS {
        assert_eq!(find_env(agent, name).as_deref(), Some(ca_file.as_str()));
    }

    let containers = &ps.containers;
    let sidecar = containers
        .iter()
        .find(|c| c.name == "egress-proxy")
        .ok_or("missing value")?;
    let sidecar_mounts = sidecar.volume_mounts.as_ref().ok_or("missing value")?;
    assert!(
        sidecar_mounts
            .iter()
            .any(|m| m.name == EGRESS_PROXY_CA_VOLUME_NAME && m.mount_path == EGRESS_PROXY_CA_MOUNT_PATH)
    );
    Ok(())
}

#[tokio::test]
async fn launch_sets_violation_nats_url_and_lease_id_on_egress_sidecar() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok, MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::AllowHosts(vec!["example.com".to_string()]))?;
    spec.network_policy = None;

    provider(mock.clone())
        .with_violation_nats_url("nats://nats.nats.svc:4222")
        .launch_with_options(
            spec,
            K8sLaunchOptions {
                lease_id: Some("lease-abc".to_string()),
                ..Default::default()
            },
        )
        .await?;

    let jobs = mock.created_jobs();
    let ps = pod_spec(&jobs[0])?;
    let sidecar = ps
        .containers
        .iter()
        .find(|c| c.name == "egress-proxy")
        .ok_or("missing value")?;
    assert_eq!(
        find_env(sidecar, "EGRESS_PROXY_VIOLATION_NATS_URL").as_deref(),
        Some("nats://nats.nats.svc:4222")
    );
    assert_eq!(find_env(sidecar, "EGRESS_PROXY_LEASE_ID").as_deref(), Some("lease-abc"));
    let init = ps
        .init_containers
        .as_ref()
        .ok_or("missing value")?
        .iter()
        .find(|c| c.name == "egress-proxy-iptables-init")
        .ok_or("missing value")?;
    let script = init.args.as_ref().ok_or("missing value")?.join(" ");
    assert!(script.contains("-m owner --uid-owner 65531 -j RETURN"));
    assert!(!script.contains("--dport 4222"));
    assert_eq!(find_env(sidecar, "EGRESS_PROXY_EXEMPT_UID").as_deref(), Some("65531"));
    assert_eq!(
        sidecar.security_context.as_ref().ok_or("missing value")?.run_as_user,
        Some(65531)
    );
    let agent_context = agent_container(&jobs[0])?
        .security_context
        .as_ref()
        .ok_or("missing value")?;
    assert_eq!(agent_context.allow_privilege_escalation, Some(false));
    let agent_caps = agent_context.capabilities.as_ref().ok_or("missing value")?;
    assert_eq!(agent_caps.drop, Some(vec!["ALL".to_string()]));
    Ok(())
}

#[tokio::test]
async fn launch_refuses_an_agent_user_that_is_the_proxy_uid() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::AllowHosts(vec!["example.com".to_string()]))?;
    spec.network_policy = None;
    spec.user = Some("65531".to_string());

    let result = provider(mock.clone()).launch(spec).await;

    assert!(result.is_err());
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn launch_with_only_pass_through_domain_rules_skips_ca_wiring() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok, MockResponse::Ok, MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::AllowHosts(vec!["example.com".to_string()]))?;
    spec.network_policy = None;

    provider(mock.clone()).launch(spec).await?;

    let jobs = mock.created_jobs();
    let ps = pod_spec(&jobs[0])?;
    let vol_names: Vec<&str> = ps
        .volumes
        .as_ref()
        .ok_or("missing value")?
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert!(
        !vol_names.contains(&EGRESS_PROXY_CA_VOLUME_NAME),
        "PassThrough-only rules must not wire a CA volume that never gets populated"
    );

    let agent = agent_container(&jobs[0])?;
    assert!(find_env(agent, "SSL_CERT_FILE").is_none());
    assert!(find_env(agent, "CURL_CA_BUNDLE").is_none());

    let sidecar = ps
        .containers
        .iter()
        .find(|c| c.name == "egress-proxy")
        .ok_or("missing value")?;
    let sidecar_env = sidecar.env.as_ref().ok_or("missing value")?;
    assert!(!sidecar_env.iter().any(|e| e.name == "EGRESS_PROXY_CA_OUT_FILE"));
    Ok(())
}

#[tokio::test]
async fn launch_applies_security_context() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let sc = agent_container(&jobs[0])?
        .security_context
        .as_ref()
        .ok_or("missing value")?;
    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.read_only_root_filesystem, Some(true));
    let drop = sc
        .capabilities
        .as_ref()
        .ok_or("missing value")?
        .drop
        .as_ref()
        .ok_or("missing value")?;
    assert!(drop.iter().any(|v| v == "ALL"));
    Ok(())
}

#[tokio::test]
async fn launch_with_options_non_privileged_is_unchanged() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.privileged = PrivilegedAccess::Denied;
    provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await?;

    let jobs = mock.created_jobs();
    let job = &jobs[0];
    let container = agent_container(job)?;
    let sc = container.security_context.as_ref().ok_or("missing value")?;
    assert_eq!(sc.read_only_root_filesystem, Some(true));
    let caps = sc.capabilities.as_ref().ok_or("missing value")?;
    assert_eq!(caps.drop, Some(vec!["ALL".to_string()]));
    assert_eq!(caps.add, None);
    assert!(
        container
            .volume_mounts
            .as_ref()
            .ok_or("missing value")?
            .iter()
            .all(|m| m.name != "fuse-device")
    );
    assert!(
        pod_spec(job)?
            .volumes
            .as_ref()
            .ok_or("missing value")?
            .iter()
            .all(|v| v.name != "fuse-device")
    );
    Ok(())
}

#[tokio::test]
async fn launch_with_options_privileged_grants_capabilities_and_fuse_device() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.privileged = PrivilegedAccess::Granted;
    provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await?;

    let jobs = mock.created_jobs();
    let job = &jobs[0];
    let container = agent_container(job)?;
    let sc = container.security_context.as_ref().ok_or("missing value")?;
    assert_eq!(sc.read_only_root_filesystem, Some(false));
    let caps = sc.capabilities.as_ref().ok_or("missing value")?;
    assert_eq!(caps.drop, Some(vec!["ALL".to_string()]));
    assert_eq!(caps.add, Some(vec!["NET_ADMIN".to_string(), "SYS_ADMIN".to_string()]));

    let mount = container
        .volume_mounts
        .as_ref()
        .ok_or("missing value")?
        .iter()
        .find(|m| m.name == "fuse-device")
        .ok_or("missing value")?;
    assert_eq!(mount.mount_path, "/dev/fuse");

    let volume = pod_spec(job)?
        .volumes
        .as_ref()
        .ok_or("missing value")?
        .iter()
        .find(|v| v.name == "fuse-device")
        .ok_or("missing value")?;
    let host_path = volume.host_path.as_ref().ok_or("missing value")?;
    assert_eq!(host_path.path, "/dev/fuse");
    assert_eq!(host_path.type_.as_deref(), Some("CharDevice"));
    Ok(())
}

#[tokio::test]
async fn launch_with_options_privileged_rejected_on_weak_isolation_runtime_class() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_runtime_class("runc"),
    );
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.privileged = PrivilegedAccess::Granted;

    let err = p
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await
        .err()
        .ok_or("missing value")?;
    assert!(matches!(err, K8sProviderError::PrivilegedNeedsStrongIsolation { .. }));
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn launch_with_options_sets_run_as_user_and_group() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.user = Some("2000:2000".to_string());
    provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await?;

    let jobs = mock.created_jobs();
    let sc = agent_container(&jobs[0])?
        .security_context
        .as_ref()
        .ok_or("missing value")?;
    assert_eq!(sc.run_as_user, Some(2000));
    assert_eq!(sc.run_as_group, Some(2000));
    assert_eq!(sc.run_as_non_root, Some(true));
    Ok(())
}

#[tokio::test]
async fn launch_with_options_rejects_root_user() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.user = Some("0".to_string());
    let err = provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await
        .err()
        .ok_or("missing value")?;
    assert!(matches!(err, K8sProviderError::RunAsUser(RunAsUserError::Root { .. })));
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn launch_with_options_rejects_root_user_with_gid() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.user = Some("0:0".to_string());
    let err = provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await
        .err()
        .ok_or("missing value")?;
    assert!(matches!(err, K8sProviderError::RunAsUser(RunAsUserError::Root { .. })));
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn launch_with_options_rejects_malformed_user() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.user = Some("notanumber".to_string());
    let err = provider(mock.clone())
        .launch_with_options(spec, K8sLaunchOptions::default())
        .await
        .err()
        .ok_or("missing value")?;
    assert!(matches!(
        err,
        K8sProviderError::RunAsUser(RunAsUserError::NotNumeric { .. })
    ));
    assert!(mock.created_jobs().is_empty());
    Ok(())
}

#[tokio::test]
async fn launch_disables_automount_service_account_token() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    assert_eq!(pod_spec(&jobs[0])?.automount_service_account_token, Some(false));
    Ok(())
}

#[tokio::test]
async fn launch_sets_image_pull_policy_if_not_present() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    assert_eq!(
        agent_container(&jobs[0])?.image_pull_policy.as_deref(),
        Some("IfNotPresent")
    );
    Ok(())
}

#[tokio::test]
async fn launch_includes_image_pull_secrets_when_configured() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let config = K8sMicroVmConfig::new("default").with_image_pull_secrets(vec!["registry-creds".to_string()]);
    let p = K8sMicroVmProvider::new(Arc::clone(&mock) as Arc<dyn K8sOps>, config);

    p.launch(sample_spec(EgressPolicy::DenyAll)?).await?;

    let jobs = mock.created_jobs();
    let secrets = pod_spec(&jobs[0])?.image_pull_secrets.as_ref().ok_or("missing value")?;
    assert!(secrets.iter().any(|s| s.name == "registry-creds"));
    Ok(())
}
