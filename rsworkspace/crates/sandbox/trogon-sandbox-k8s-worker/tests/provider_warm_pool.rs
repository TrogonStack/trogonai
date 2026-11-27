mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use support::*;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{Digest, NetworkPolicy};
use trogon_sandbox_k8s_worker::{DigestCache, WarmPool, WarmPoolConfig};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_providers::{K8sMicroVmConfig, LABEL_LEASE_DEADLINE, LABEL_LEASE_ID, LABEL_WARM_POOL};

const REPOSITORY: &str = "warm/repo";

fn settings() -> WarmPoolConfig {
    WarmPoolConfig {
        max_pool_size_per_digest: 2,
        max_total_warm: 4,
        max_idle_seconds: 600,
        refill_per_pass: 2,
        max_warming: 4,
    }
}

fn digest_of(bytes: &[u8]) -> Boxed<Digest> {
    let hex: String = Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(Digest::new(format!("sha256:{hex}"))?)
}

/// A world with a warm pool, the pool, and the digest cache they share.
async fn pooled(config: WarmPoolConfig) -> Boxed<(World, Arc<WarmPool>, Arc<DigestCache>)> {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let world = World::with(K8sMicroVmConfig::new(NAMESPACE), move |parts| {
        *sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(parts.digests.clone());
        Some(Arc::new(WarmPool::new(
            parts.inner.clone(),
            parts.oci.clone(),
            parts.digests.clone(),
            config,
            IMAGE.into(),
            parts.registry.clone(),
            REPOSITORY.into(),
            "gvisor".into(),
        )))
    })
    .await?;
    let digests = captured
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .ok_or("the pool was not built")?;
    let pool = world.provider.warm_pool.clone().ok_or("the provider has no pool")?;
    Ok((world, pool, digests))
}

/// Starts every pod the fake holds.
fn run_pods(world: &World) {
    let mut state = world.cluster.state();
    for ((kind, _), pod) in state.objects.iter_mut() {
        if kind == "pods" {
            pod["status"] = json!({"phase": "Running"});
        }
    }
}

fn edit_pods(world: &World, change: impl Fn(&str, &mut Value)) {
    let mut state = world.cluster.state();
    for ((kind, name), pod) in state.objects.iter_mut() {
        if kind == "pods" {
            change(name, pod);
        }
    }
}

#[tokio::test]
async fn the_pool_fills_and_serves_claims_and_leases() -> TestResult {
    let (world, pool, digests) = pooled(settings()).await?;
    let artifact = seed_snapshot(&world.registry, REPOSITORY, b"rootfs").await?;
    digests.insert(artifact.digest.clone());

    pool.reconcile().await?;
    assert_eq!(world.cluster.names("pods").len(), 2);
    assert_eq!(world.cluster.names("secrets").len(), 2);
    assert!(pool.population().await.is_empty(), "pods still starting are not ready");
    pool.reconcile().await?;
    assert_eq!(
        world.cluster.names("pods").len(),
        2,
        "starting pods count toward the pool"
    );

    run_pods(&world);
    pool.reconcile().await?;
    assert_eq!(pool.population().await.get(&artifact.digest), Some(&2));

    let unknown = digest_of(b"unknown")?;
    assert!(
        pool.try_claim(
            &unknown,
            "lease-none",
            "tenant-a",
            &NetworkPolicy::AllowAll,
            Duration::ZERO
        )
        .await?
        .is_none()
    );
    let claim = pool
        .try_claim(
            &artifact.digest,
            "lease-warm",
            "tenant-a",
            &NetworkPolicy::DenyAll,
            Duration::from_secs(60),
        )
        .await?
        .ok_or("a warm pod should be claimed")?;
    assert!(claim.network_policy_name.is_some());
    assert!(claim.secret_name.is_some());
    let claimed = world
        .cluster
        .object("pods", &claim.pod_name)
        .ok_or("the claimed pod is gone")?;
    assert_eq!(claimed["metadata"]["labels"][LABEL_LEASE_ID], "lease-warm");
    assert!(claimed["metadata"]["labels"][LABEL_LEASE_DEADLINE].is_string());
    assert_eq!(pool.population().await.get(&artifact.digest), Some(&1));

    let request = lease_request_with(json!({
        "image": {"kind": "snapshot", "reference": serde_json::to_value(&artifact)?},
        "memory_mib": 128,
    }))?;
    let grant = world.provider.lease(&request, &lease_id("lease-served")).await?;
    assert_eq!(world.cluster.state().jobs_created, 0, "a warm pod serves the lease");
    assert!(world.cluster.names("pods").contains(&grant.sandbox_id.to_string()));
    assert!(pool.population().await.is_empty());

    let large =
        lease_request_with(json!({"image": {"kind": "snapshot", "reference": serde_json::to_value(&artifact)?}}))?;
    world.provider.lease(&large, &lease_id("lease-large")).await?;
    assert_eq!(
        world.cluster.state().jobs_created,
        1,
        "a request larger than a warm pod cold starts"
    );

    assert!(pool.fits(None, 128));
    assert!(!pool.fits(Some(500), 128));
    assert!(!pool.fits(None, 512));
    Ok(())
}

#[tokio::test]
async fn idle_pods_past_their_age_are_evicted_by_the_apiserver_clock() -> TestResult {
    let (world, pool, digests) = pooled(WarmPoolConfig {
        max_idle_seconds: 10,
        ..settings()
    })
    .await?;
    let artifact = seed_snapshot(&world.registry, REPOSITORY, b"rootfs").await?;
    digests.insert(artifact.digest.clone());
    pool.reconcile().await?;
    run_pods(&world);
    let original = world.cluster.names("pods");

    world.cluster.state().no_clock = true;
    pool.reconcile().await?;
    assert_eq!(
        world.cluster.names("pods"),
        original,
        "without a clock nothing is evicted"
    );
    world.cluster.state().no_clock = false;

    let claim = pool
        .try_claim(
            &artifact.digest,
            "lease-held",
            "tenant-a",
            &NetworkPolicy::AllowAll,
            Duration::ZERO,
        )
        .await?
        .ok_or("a warm pod should be claimed")?;
    let idle = original
        .iter()
        .find(|name| **name != claim.pod_name)
        .ok_or("an idle pod remains")?
        .clone();
    edit_pods(&world, |name, pod| {
        if *name == idle
            && let Some(metadata) = pod["metadata"].as_object_mut()
        {
            metadata.remove("creationTimestamp");
        }
    });
    let mut ghost = world.cluster.object("pods", &idle).ok_or("the idle pod is gone")?;
    if let Some(metadata) = ghost["metadata"].as_object_mut() {
        metadata.remove("name");
    }
    world
        .cluster
        .state()
        .objects
        .insert(("pods".into(), "ghost".into()), ghost);
    pool.reconcile().await?;
    assert!(
        world.cluster.object("pods", &idle).is_some(),
        "a pod with no age is kept"
    );
    assert!(
        world.cluster.object("pods", &claim.pod_name).is_some(),
        "a claimed pod is kept"
    );

    edit_pods(&world, |name, pod| {
        if *name == idle {
            pod["metadata"]["creationTimestamp"] = json!(CREATED_AT);
        }
    });
    world
        .cluster
        .state()
        .objects
        .remove(&("pods".to_string(), "ghost".to_string()));
    pool.reconcile().await?;
    assert!(
        world.cluster.object("pods", &idle).is_none(),
        "an old idle pod is evicted"
    );
    assert!(world.cluster.object("pods", &claim.pod_name).is_some());
    Ok(())
}

#[tokio::test]
async fn failures_are_retained_and_retried() -> TestResult {
    let (world, pool, digests) = pooled(settings()).await?;
    let artifact = seed_snapshot(&world.registry, REPOSITORY, b"rootfs").await?;
    digests.insert(artifact.digest.clone());
    digests.insert(digest_of(b"not in the registry")?);

    world.cluster.state().fail_reads.push("pods".into());
    assert!(pool.reconcile().await.is_err());
    world.cluster.state().fail_reads.clear();

    world.cluster.state().fail_creates.push("secrets".into());
    pool.reconcile().await?;
    assert!(world.cluster.names("pods").is_empty());
    world.cluster.state().fail_creates.clear();

    {
        let mut state = world.cluster.state();
        state.fail_creates.push("pods".into());
        state.fail_deletes = 100;
    }
    pool.reconcile().await?;
    assert!(
        !world.cluster.names("secrets").is_empty(),
        "the orphaned secret awaits cleanup"
    );
    pool.reconcile().await?;
    world.cluster.state().fail_creates.clear();
    world.cluster.state().fail_deletes = 0;
    pool.reconcile().await?;
    assert_eq!(world.cluster.names("pods").len(), 2);
    assert_eq!(
        world.cluster.names("secrets").len(),
        2,
        "the retried cleanup removed the orphan"
    );

    edit_pods(&world, |_, pod| pod["status"] = json!({"phase": "Failed"}));
    world.cluster.state().fail_deletes = 100;
    pool.reconcile().await?;
    world.cluster.state().fail_deletes = 0;
    pool.reconcile().await?;
    assert!(
        world
            .cluster
            .state()
            .calls
            .iter()
            .any(|call| call.method == "DELETE" && call.path.contains("/pods/"))
    );

    run_pods(&world);
    pool.reconcile().await?;
    world.cluster.state().fail_creates.push("networkpolicies".into());
    let refused = pool
        .try_claim(
            &artifact.digest,
            "lease-refused",
            "tenant-a",
            &NetworkPolicy::DenyAll,
            Duration::ZERO,
        )
        .await;
    assert!(refused.is_err());
    world.cluster.state().fail_deletes = 100;
    let retained = pool
        .try_claim(
            &artifact.digest,
            "lease-retained",
            "tenant-a",
            &NetworkPolicy::DenyAll,
            Duration::ZERO,
        )
        .await;
    assert!(retained.is_err());
    world.cluster.state().fail_deletes = 0;
    world.cluster.state().fail_creates.clear();
    pool.reconcile().await?;
    Ok(())
}

#[tokio::test]
async fn the_controller_reconciles_until_cancelled() -> TestResult {
    let (world, pool, digests) = pooled(settings()).await?;
    let artifact = seed_snapshot(&world.registry, REPOSITORY, b"rootfs").await?;
    digests.insert(artifact.digest.clone());
    world.cluster.state().fail_reads.push("pods".into());
    let cancel = CancellationToken::new();
    pool.clone().spawn_controller(cancel.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        world.cluster.names("pods").is_empty(),
        "a failing reconcile creates nothing"
    );
    cancel.cancel();

    world.cluster.state().fail_reads.clear();
    let second = CancellationToken::new();
    pool.spawn_controller(second.clone());
    for _ in 0..100 {
        if world.cluster.names("pods").len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(world.cluster.names("pods").len(), 2);
    second.cancel();
    Ok(())
}

#[tokio::test]
async fn a_restarted_pool_relearns_its_sources_from_the_pods() -> TestResult {
    let (world, pool, digests) = pooled(settings()).await?;
    let artifact = seed_snapshot(&world.registry, REPOSITORY, b"rootfs").await?;
    digests.insert(artifact.digest.clone());
    pool.reconcile().await?;
    run_pods(&world);
    let other = pool.source_for(&digest_of(b"never seen")?);
    assert_eq!(
        other.repository, REPOSITORY,
        "an unseen digest falls back to the configured repository"
    );

    let fresh = Arc::new(DigestCache::new(8));
    let restarted = WarmPool::new(
        world.cluster.provider()?,
        Arc::new(world.registry.client()?),
        fresh.clone(),
        settings(),
        IMAGE.into(),
        world.registry.address.clone(),
        "elsewhere".into(),
        "gvisor".into(),
    );
    let names = world.cluster.names("pods");
    let [first, second] = names.as_slice() else {
        return Err("two pods expected".into());
    };
    edit_pods(&world, |name, pod| {
        if name == second
            && let Some(annotations) = pod["metadata"]["annotations"].as_object_mut()
        {
            annotations.retain(|key, _| !key.contains("source"));
        }
        if name == first
            && let Some(annotations) = pod["metadata"]["annotations"].as_object_mut()
        {
            for (key, value) in annotations.iter_mut() {
                if key.contains("source-repository") {
                    *value = json!("");
                }
            }
        }
    });
    restarted.reconcile().await?;
    assert!(fresh.contains(&artifact.digest));
    assert_eq!(restarted.source_for(&artifact.digest).repository, "elsewhere");

    edit_pods(&world, |_, pod| {
        pod["metadata"]["labels"][LABEL_WARM_POOL] = json!("idle");
    });
    let names = world.cluster.names("pods");
    let Some(only) = names.first() else {
        return Err("a pod remains".into());
    };
    let source = world.cluster.object("pods", only).ok_or("the pod is gone")?;
    assert!(source["metadata"]["annotations"].is_object());
    Ok(())
}
