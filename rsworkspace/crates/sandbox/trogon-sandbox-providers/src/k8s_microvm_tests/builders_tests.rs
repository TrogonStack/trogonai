use super::*;

#[test]
fn a_tier_with_its_own_runtime_class_gets_it_and_every_other_gets_the_default() -> TestResult {
    let config = K8sMicroVmConfig::new("default")
        .with_runtime_class("kata-fc")
        .with_runtime_class_for_tier(IsolationTier::FullVm, "kata-qemu");

    assert_eq!(config.runtime_class_for(Some(IsolationTier::FullVm)), "kata-qemu");
    assert_eq!(config.runtime_class_for(Some(IsolationTier::MicroVm)), "kata-fc");
    assert_eq!(config.runtime_class_for(None), "kata-fc");
    Ok(())
}

#[test]
fn the_config_builders_set_the_service_account_and_resource_naming() -> TestResult {
    let config = K8sMicroVmConfig::new("work")
        .with_service_account("agents")
        .with_resource_naming(K8sResourceNaming::Legacy);

    assert_eq!(config.namespace, "work");
    assert_eq!(config.service_account, "agents");
    assert_eq!(config.resource_naming, K8sResourceNaming::Legacy);
    Ok(())
}

#[tokio::test]
async fn the_provider_reports_its_namespace_and_resolves_runtime_classes_through_its_config() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let provider = K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("work").with_runtime_class_for_tier(IsolationTier::FullVm, "kata-qemu"),
    );

    assert_eq!(provider.namespace(), "work");
    assert_eq!(provider.runtime_class_for(Some(IsolationTier::FullVm)), "kata-qemu");
    assert_eq!(provider.runtime_class_for(None), "kata-fc");
    Ok(())
}

#[tokio::test]
async fn a_scoped_provider_keeps_the_configuration_and_swaps_the_lifecycle_emitter() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let base = provider(mock.clone());

    let scoped = base
        .with_lifecycle_emitter(None)
        .with_runtime_class_override("kata-qemu");
    scoped.launch(sample_spec(EgressPolicy::DenyAll)?).await?;

    assert_eq!(scoped.namespace(), "default");
    let jobs = mock.created_jobs();
    assert_eq!(
        pod_spec(&jobs[0])?.runtime_class_name.as_deref(),
        Some("kata-qemu"),
        "the override travels with the scoped provider"
    );
    Ok(())
}

#[tokio::test]
async fn a_provider_builds_over_a_real_client_and_a_nats_context() -> TestResult {
    let config = kube::Config::new("http://127.0.0.1:1".parse()?);
    let client = kube::Client::try_from(config)?;
    let nats = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect("nats://127.0.0.1:1")
        .await?;

    let provider = K8sMicroVmProvider::with_client(client, K8sMicroVmConfig::new("work"))
        .with_nats(async_nats::jetstream::new(nats));

    assert_eq!(provider.namespace(), "work");
    Ok(())
}

#[tokio::test]
async fn the_default_client_either_builds_or_reports_that_it_could_not() -> TestResult {
    let result = K8sMicroVmProvider::with_default_client(K8sMicroVmConfig::new("work")).await;

    assert!(
        matches!(result, Ok(_) | Err(K8sProviderError::BuildClient(_))),
        "unexpected failure building the default client"
    );
    Ok(())
}
