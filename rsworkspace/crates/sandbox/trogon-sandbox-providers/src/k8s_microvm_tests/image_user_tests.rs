use super::*;

#[tokio::test]
async fn keep_alive_overrides_the_agent_command() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch_with_options(
            sample_spec(EgressPolicy::DenyAll)?,
            K8sLaunchOptions {
                keep_alive: true,
                ..Default::default()
            },
        )
        .await?;

    let jobs = mock.created_jobs();
    let agent = pod_spec(&jobs[0])?
        .containers
        .iter()
        .find(|c| c.name == "agent")
        .ok_or("missing value")?
        .clone();
    let command = agent.command.ok_or("missing value")?;
    assert_eq!(command[..2], ["/bin/sh".to_string(), "-c".to_string()]);
    assert!(
        command[2].contains("trap 'exit 0' TERM INT QUIT"),
        "keep-alive must exit on SIGTERM and an image's SIGQUIT stop signal: {command:?}"
    );
    assert_eq!(
        pod_spec(&jobs[0])?.termination_grace_period_seconds,
        Some(SANDBOX_TERMINATION_GRACE_SECONDS)
    );
    Ok(())
}

#[tokio::test]
async fn an_agent_with_no_user_runs_as_the_default_non_root_uid() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    provider(mock.clone())
        .launch(sample_spec(EgressPolicy::DenyAll)?)
        .await?;

    let jobs = mock.created_jobs();
    let pod = pod_spec(&jobs[0])?;
    let agent = pod
        .containers
        .iter()
        .find(|c| c.name == "agent")
        .ok_or("missing value")?;
    let sc = agent.security_context.as_ref().ok_or("missing value")?;
    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.run_as_user, Some(DEFAULT_AGENT_UID));
    assert_eq!(sc.run_as_group, Some(DEFAULT_AGENT_UID));
    assert_eq!(
        pod.security_context.as_ref().ok_or("missing value")?.fs_group,
        Some(WORKSPACE_FS_GROUP)
    );
    Ok(())
}

#[tokio::test]
async fn a_root_image_user_falls_back_to_the_default_uid_when_honoring() -> TestResult {
    for image_user in [Some("root"), Some("0"), Some("0:0"), Some(""), None] {
        let mock = MockK8sOps::new(vec![MockResponse::Ok]);
        let provider = honoring_provider(&mock, Ok(image_user.map(str::to_string)));

        let sc = launched_agent_security_context(&mock, provider).await?;

        assert_eq!(sc.run_as_non_root, Some(true), "{image_user:?}");
        assert_eq!(sc.run_as_user, Some(DEFAULT_AGENT_UID), "{image_user:?}");
        assert_eq!(sc.run_as_group, Some(DEFAULT_AGENT_UID), "{image_user:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_unreadable_image_user_falls_back_to_the_default_uid_when_honoring() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let provider = honoring_provider(&mock, Err("registry unreachable".to_string()));

    let sc = launched_agent_security_context(&mock, provider).await?;

    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.run_as_user, Some(DEFAULT_AGENT_UID));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stalled_image_user_lookup_falls_back_to_the_default_uid() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let provider = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_honor_image_user(true),
    )
    .with_image_user_resolver(Arc::new(NeverAnswers));

    let sc = launched_agent_security_context(&mock, provider).await?;

    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.run_as_user, Some(DEFAULT_AGENT_UID));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cached_image_user_skips_the_second_lookup_until_it_expires() -> TestResult {
    let resolver = Arc::new(CountingImageUser(
        std::sync::atomic::AtomicUsize::new(0),
        Ok(Some("1000".to_string())),
    ));
    let provider = K8sMicroVmProvider::new(
        MockK8sOps::new(vec![]) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_honor_image_user(true),
    )
    .with_image_user_resolver(Arc::clone(&resolver) as Arc<dyn ImageUserResolver>);

    assert_eq!(provider.agent_ids_for_image("img:1").await, (None, None));
    assert_eq!(provider.agent_ids_for_image("img:1").await, (None, None));
    assert_eq!(resolver.0.load(Ordering::SeqCst), 1);

    provider.agent_ids_for_image("img:2").await;
    assert_eq!(resolver.0.load(Ordering::SeqCst), 2);

    tokio::time::advance(IMAGE_USER_CACHE_TTL + std::time::Duration::from_secs(1)).await;
    provider.agent_ids_for_image("img:1").await;
    assert_eq!(resolver.0.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_image_user_lookup_is_not_cached() -> TestResult {
    let resolver = Arc::new(CountingImageUser(
        std::sync::atomic::AtomicUsize::new(0),
        Err("registry unreachable".to_string()),
    ));
    let provider = K8sMicroVmProvider::new(
        MockK8sOps::new(vec![]) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_honor_image_user(true),
    )
    .with_image_user_resolver(Arc::clone(&resolver) as Arc<dyn ImageUserResolver>);

    provider.agent_ids_for_image("img:1").await;
    provider.agent_ids_for_image("img:1").await;

    assert_eq!(resolver.0.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn honoring_without_an_image_lookup_falls_back_to_the_default_uid() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let provider = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_honor_image_user(true),
    );

    let sc = launched_agent_security_context(&mock, provider).await?;

    assert_eq!(sc.run_as_user, Some(DEFAULT_AGENT_UID));
    Ok(())
}

#[tokio::test]
async fn an_agent_with_no_user_keeps_the_image_user_when_told_to() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let provider = honoring_provider(&mock, Ok(Some("1000:1000".to_string())));

    provider.launch(sample_spec(EgressPolicy::DenyAll)?).await?;

    let jobs = mock.created_jobs();
    let agent = pod_spec(&jobs[0])?
        .containers
        .iter()
        .find(|c| c.name == "agent")
        .ok_or("missing value")?
        .clone();
    let sc = agent.security_context.ok_or("missing value")?;
    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.run_as_user, None);
    assert_eq!(sc.run_as_group, None);
    Ok(())
}

#[tokio::test]
async fn a_lease_naming_a_user_overrides_honoring_the_image_user() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let provider = honoring_provider(&mock, Ok(Some("1000:1000".to_string())));
    let mut spec = sample_spec(EgressPolicy::DenyAll)?;
    spec.user = Some("2000:3000".to_string());

    provider.launch(spec).await?;

    let jobs = mock.created_jobs();
    let agent = pod_spec(&jobs[0])?
        .containers
        .iter()
        .find(|c| c.name == "agent")
        .ok_or("missing value")?
        .clone();
    let sc = agent.security_context.ok_or("missing value")?;
    assert_eq!(sc.run_as_user, Some(2000));
    assert_eq!(sc.run_as_group, Some(3000));
    assert_eq!(sc.run_as_non_root, Some(true));
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_keeps_the_image_user_when_told_to() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let provider = honoring_provider(&mock, Ok(Some("1000:1000".to_string())));

    provider
        .create_warm_pod("sha256:abc", "img:1", "restore", "kata-fc", 0.25, 256)
        .await?;

    let pods = mock.created_pods();
    let agent = pods[0]
        .spec
        .as_ref()
        .ok_or("missing value")?
        .containers
        .iter()
        .find(|c| c.name == "agent")
        .ok_or("missing value")?;
    let sc = agent.security_context.as_ref().ok_or("missing value")?;
    assert_eq!(sc.run_as_non_root, Some(true));
    assert_eq!(sc.run_as_user, None);
    Ok(())
}
