//! Warm pool reuse for snapshot leases.

mod docker_support;
mod zot_support;

use std::time::{Duration, Instant};

use docker_support::{TestResult, fresh_lease_id};
use trogon_sandbox_contract::{Digest, LeaseRequest, OciArtifactRef};
use trogon_sandbox_docker_worker::adapter::DockerWorkerProvider;
use trogon_sandbox_docker_worker::warm_pool::{WarmContainer, WarmPool, WarmPoolConfig};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_providers::{InspectStatus, WORKSPACE_MOUNT_PATH};
use zot_support::{Zot, build_registry_provider, push_test_snapshot, snapshot_lease_request};

async fn lease_and_release(provider: &DockerWorkerProvider, req: &LeaseRequest) -> TestResult<Duration> {
    let lease_id = fresh_lease_id();
    let started = Instant::now();
    let lease = provider.lease(req, &lease_id).await?;
    let elapsed = started.elapsed();

    let status = provider.inner.engine().inspect(lease.sandbox_id.as_str()).await?;
    assert_eq!(status, InspectStatus::Running);

    provider.cancel(&lease_id).await?;
    Ok(elapsed)
}

fn pool_config(max_per_digest: usize) -> WarmPoolConfig {
    WarmPoolConfig {
        enabled: true,
        max_pool_size_per_digest: max_per_digest,
        max_total_warm: 16,
        max_idle_seconds: 600,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn warm_pool_reuses_containers_for_same_digest() -> TestResult {
    let zot = Zot::start().await?;
    let artifact_ref = push_test_snapshot(&zot, "snapshots/test/warm-pool").await?;
    let provider = build_registry_provider(&zot, pool_config(3)).await?;
    let req = snapshot_lease_request(&artifact_ref, "warm-pool-test")?;

    let cold_latency = lease_and_release(&provider, &req).await?;
    let population = provider.warm_pool_population().await;
    assert_eq!(
        population.get(&artifact_ref.digest),
        Some(&3),
        "pool should refill to max_per_digest after first lease release"
    );

    let mut warm_latencies = Vec::new();
    for _ in 0..3 {
        warm_latencies.push(lease_and_release(&provider, &req).await?);
    }

    assert_eq!(provider.warm_grab_count(), 3);
    assert_eq!(provider.cold_start_count(), 1);

    let warm_best = warm_latencies.iter().copied().min().ok_or("warm latencies")?;
    assert!(
        warm_best < cold_latency,
        "warm grab ({warm_best:?}) should beat cold start ({cold_latency:?})"
    );

    let population = provider.warm_pool_population().await;
    assert_eq!(
        population.get(&artifact_ref.digest),
        Some(&3),
        "pool should refill after warm leases complete"
    );

    for _ in 0..3 {
        provider.warm_pool().lock().await.take(&artifact_ref.digest);
    }
    assert_eq!(provider.warm_pool_population().await.get(&artifact_ref.digest), None);

    lease_and_release(&provider, &req).await?;
    assert_eq!(provider.cold_start_count(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_stops_the_container() -> TestResult {
    let zot = Zot::start().await?;
    let artifact_ref = push_test_snapshot(&zot, "snapshots/test/warm-pool-snapshot").await?;
    let provider = build_registry_provider(&zot, WarmPoolConfig::default()).await?;

    let req = snapshot_lease_request(&artifact_ref, "warm-pool-test")?;
    let lease_id = fresh_lease_id();
    let lease = provider.lease(&req, &lease_id).await?;
    assert_eq!(
        provider.inner.engine().inspect(lease.sandbox_id.as_str()).await?,
        InspectStatus::Running
    );

    let target = OciArtifactRef {
        registry: zot.registry.clone(),
        repository: "snapshots/test/manual-snapshot".to_string(),
        digest: artifact_ref.digest.clone(),
        artifact_type: artifact_ref.artifact_type,
    };
    let receipt = provider.snapshot(&lease_id, &target).await?;
    assert!(receipt.bytes > 0);

    let post_snapshot_status = provider.inner.engine().inspect(lease.sandbox_id.as_str()).await;
    assert!(
        !matches!(post_snapshot_status, Ok(InspectStatus::Running)),
        "container must not still be running after a manual snapshot"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn warm_pool_reset_does_not_leak_state_across_tenants() -> TestResult {
    let zot = Zot::start().await?;
    let artifact_ref = push_test_snapshot(&zot, "snapshots/test/warm-pool-reset").await?;
    let provider = build_registry_provider(&zot, pool_config(1)).await?;

    let req_a = snapshot_lease_request(&artifact_ref, "warm-pool-test-tenant-a")?;
    let lease_a_id = fresh_lease_id();
    let lease_a = provider.lease(&req_a, &lease_a_id).await?;

    let marker_path = format!("{WORKSPACE_MOUNT_PATH}/tenant-a-secret.txt");
    let write_status = tokio::process::Command::new("docker")
        .args([
            "exec",
            lease_a.sandbox_id.as_str(),
            "sh",
            "-c",
            &format!("mkdir -p {WORKSPACE_MOUNT_PATH} && echo tenant-a-data > {marker_path}"),
        ])
        .status()
        .await?;
    assert!(
        write_status.success(),
        "writing marker file into tenant a workspace should succeed"
    );

    provider.cancel(&lease_a_id).await?;

    let req_b = snapshot_lease_request(&artifact_ref, "warm-pool-test-tenant-b")?;
    let lease_b_id = fresh_lease_id();
    let lease_b = provider.lease(&req_b, &lease_b_id).await?;

    assert_eq!(
        lease_b.sandbox_id, lease_a.sandbox_id,
        "tenant b should reuse the same warm-pool container tenant a released, not a fresh one"
    );

    let check_status = tokio::process::Command::new("docker")
        .args([
            "exec",
            lease_b.sandbox_id.as_str(),
            "sh",
            "-c",
            &format!("test -e {marker_path}"),
        ])
        .status()
        .await?;
    assert!(
        !check_status.success(),
        "tenant a's file must not be visible in tenant b's workspace after reset"
    );

    provider.cancel(&lease_b_id).await?;
    Ok(())
}

#[tokio::test]
async fn warm_pool_population_tracks_idle_containers() -> TestResult {
    let mut pool = WarmPool::new(WarmPoolConfig {
        max_pool_size_per_digest: 2,
        ..WarmPoolConfig::default()
    });
    let digest = Digest::new(format!("sha256:{}", "b".repeat(64)))?;
    pool.insert(WarmContainer::new(
        "c1".to_string(),
        digest.clone(),
        "img:1".to_string(),
    ));
    let pop = pool.population();
    assert_eq!(pop.get(&digest), Some(&1));
    Ok(())
}
