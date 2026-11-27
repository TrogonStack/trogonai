mod support;

use serde_json::json;
use support::*;
use trogon_sandbox_contract::LeaseError;
use trogon_sandbox_k8s_worker::K8sWorkerError;
use trogon_sandbox_protocol::{Provider, ProviderError};
use trogon_sandbox_providers::K8sMicroVmConfig;

/// The worker error a provider refusal carries, when it carries one.
fn worker_error(error: &ProviderError) -> Option<&K8sWorkerError> {
    match error {
        ProviderError::Backend(source) => source.downcast_ref(),
        _ => None,
    }
}

fn identified() -> K8sMicroVmConfig {
    K8sMicroVmConfig::new(NAMESPACE).with_worker_identity("worker-0")
}

fn snapshot_image(reference: &trogon_sandbox_contract::OciArtifactRef) -> Result<serde_json::Value, serde_json::Error> {
    Ok(json!({"kind": "snapshot", "reference": serde_json::to_value(reference)?}))
}

#[tokio::test]
async fn requests_the_worker_cannot_honor_are_refused() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-refused");
    let privileged = lease_request_with(json!({"privileged": true}))?;
    let error = world.provider.lease(&privileged, &lease).await.err().ok_or("granted")?;
    assert!(
        matches!(worker_error(&error), Some(K8sWorkerError::PrivilegedUnsupported(_))),
        "{error}"
    );

    let hostnames = lease_request_with(json!({"network_policy": {"user_defined": {
        "rules": [{"domain": "example.com", "transform": []}], "allow_ranges": [], "deny_ranges": []}}}))?;
    let error = world.provider.lease(&hostnames, &lease).await.err().ok_or("granted")?;
    assert!(
        matches!(
            worker_error(&error),
            Some(K8sWorkerError::Lease(LeaseError::InvalidNetworkPolicy { .. }))
        ),
        "{error}"
    );
    assert!(!world.provider.holds_lease(&lease).await);
    assert_eq!(world.cluster.state().jobs_created, 0);
    Ok(())
}

#[tokio::test]
async fn a_held_lease_cannot_be_granted_twice_and_is_stored_when_asked() -> TestResult {
    let world = World::persisting(identified()).await?;
    let lease = lease_id("lease-twice");
    let granted = world.provider.lease(&lease_request()?, &lease).await?;
    assert_eq!(
        granted.worker_id.as_ref().map(|worker| worker.as_str()),
        Some("worker-0")
    );
    assert!(!world.cluster.names("secrets").is_empty());
    let again = world
        .provider
        .lease(&lease_request()?, &lease)
        .await
        .err()
        .ok_or("granted twice")?;
    assert!(
        matches!(worker_error(&again), Some(K8sWorkerError::LeaseAlreadyHeld)),
        "{again}"
    );
    assert_eq!(world.provider.active_lease_count().await, 1);
    world.provider.cancel(&lease).await?;
    assert_eq!(world.provider.active_lease_count().await, 0);
    assert!(world.cluster.names("secrets").is_empty());
    Ok(())
}

#[tokio::test]
async fn a_snapshot_image_is_restored_from_the_registry() -> TestResult {
    let world = World::persisting(identified()).await?;
    let lease = lease_id("lease-restore");
    let artifact = seed_snapshot(&world.registry, "snapshots/a", b"rootfs-bytes").await?;
    let request = lease_request_with(json!({"image": snapshot_image(&artifact)?}))?;
    world.provider.lease(&request, &lease).await?;
    let secrets = world.cluster.names("secrets");
    assert!(secrets.iter().any(|name| name.contains("restore")), "{secrets:?}");
    assert!(world.provider.digest_cache.contains(&artifact.digest));
    world.provider.cancel(&lease).await?;

    let missing = trogon_sandbox_contract::OciArtifactRef {
        digest: trogon_sandbox_contract::Digest::new(format!("sha256:{}", "0".repeat(64)))?,
        ..artifact
    };
    let request = lease_request_with(json!({"image": snapshot_image(&missing)?}))?;
    assert!(
        world
            .provider
            .lease(&request, &lease_id("lease-missing"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn failed_allocations_keep_what_could_not_be_released_until_cancelled() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-failing");
    let artifact = seed_snapshot(&world.registry, "snapshots/a", b"rootfs-bytes").await?;
    let request = lease_request_with(json!({"image": snapshot_image(&artifact)?}))?;
    {
        let mut state = world.cluster.state();
        state.fail_creates.push("jobs".into());
        state.fail_deletes = 100;
    }
    assert!(world.provider.lease(&request, &lease).await.is_err());
    let retry = world.provider.lease(&request, &lease).await.err().ok_or("granted")?;
    assert!(
        matches!(worker_error(&retry), Some(K8sWorkerError::CleanupPending)),
        "{retry}"
    );
    assert_eq!(world.provider.authority_held_leases().await?.len(), 1);

    {
        let mut state = world.cluster.state();
        state.fail_creates.clear();
        state.fail_deletes = 0;
    }
    world.provider.cancel(&lease).await?;
    assert!(world.provider.authority_held_leases().await?.is_empty());
    assert!(world.provider.lease(&request, &lease).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn s3_workspaces_fail_without_a_reachable_bucket() -> TestResult {
    let world = World::start().await?;
    let request = lease_request_with(json!({"workspace": {"s3_volume": {
        "bucket": "fixture-bucket", "prefix": "work", "mode": "read_only"}}}))?;
    assert!(world.provider.lease(&request, &lease_id("lease-s3")).await.is_err());
    Ok(())
}
