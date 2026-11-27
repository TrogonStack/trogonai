mod support;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use support::*;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{LeaseAttemptIdentity, ProviderKind};
use trogon_sandbox_protocol::{AuthorityResourceId, FreezeOutcome, HeldLease, Provider, PurgeOutcome, ThawOutcome};
use trogon_sandbox_providers::K8sMicroVmConfig;

fn identified() -> K8sMicroVmConfig {
    K8sMicroVmConfig::new(NAMESPACE).with_worker_identity("worker-0")
}

/// Marks every Job in the fake cluster as finished, so its lease counts as expired.
fn finish_jobs(world: &World) {
    let mut state = world.cluster.state();
    for ((kind, _), job) in state.objects.iter_mut() {
        if kind == "jobs" {
            job["status"] = json!({"conditions": [{"type": "Complete", "status": "True"}]});
        }
    }
}

fn held(lease: &str, key: &str) -> Boxed<HeldLease> {
    Ok(HeldLease::Provider {
        lease_id: lease_id(lease),
        provider: ProviderKind::K8sMicroVm,
        resource_id: AuthorityResourceId::new(key)?,
    })
}

#[tokio::test]
async fn recovery_adopts_live_leases_and_sweeps_expired_ones() -> TestResult {
    let world = World::start().await?;
    let config = identified();
    let first = world.restarted(config.clone(), false).await?;
    assert_eq!(first.recover_leases().await?.cold, 0);

    first.lease(&lease_request()?, &lease_id("lease-live")).await?;
    let second = world.restarted(config.clone(), false).await?;
    let adopted = second.recover_leases().await?;
    assert_eq!((adopted.cold, adopted.swept), (1, 0));
    assert_eq!(
        second.recover_leases().await?.cold,
        0,
        "a lease already held is not recorded twice"
    );
    assert!(second.holds_lease(&lease_id("lease-live")).await);

    world.cluster.state().pod_phase = Some("Pending".into());
    first.lease(&lease_request()?, &lease_id("lease-starting")).await.ok();
    world.cluster.state().pod_phase = None;

    finish_jobs(&world);
    let third = world.restarted(config.clone(), false).await?;
    world.cluster.state().fail_deletes = 100;
    assert_eq!(third.recover_leases().await?.swept, 0);
    assert!(
        !third.authority_held_leases().await?.is_empty(),
        "failed cleanup stays recorded"
    );
    world.cluster.state().fail_deletes = 0;
    assert_eq!(third.sweep_expired_leases().await?, 1);
    assert!(third.authority_held_leases().await?.is_empty());
    assert!(world.cluster.names("jobs").is_empty());

    let fourth = world.restarted(config, false).await?;
    assert_eq!(fourth.recover_leases().await?.swept, 0);
    Ok(())
}

#[tokio::test]
async fn sweeping_tears_down_expired_leases_and_pending_cleanup() -> TestResult {
    let world = World::start().await?;
    let provider = world.restarted(identified(), false).await?;
    provider.lease(&lease_request()?, &lease_id("lease-expired")).await?;
    provider.lease(&lease_request()?, &lease_id("lease-fine")).await?;
    assert_eq!(provider.sweep_expired_leases().await?, 0);

    finish_jobs(&world);
    assert_eq!(provider.sweep_expired_leases().await?, 2);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn the_sweeper_runs_until_cancelled() -> TestResult {
    let world = World::start().await?;
    let provider = world.restarted(identified(), false).await?;
    provider.lease(&lease_request()?, &lease_id("lease-sweep")).await?;
    let cancel = CancellationToken::new();
    Arc::clone(&provider).spawn_lease_sweeper(Duration::from_millis(30), cancel.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(provider.holds_lease(&lease_id("lease-sweep")).await);

    world.cluster.state().fail_reads.push("jobs".into());
    tokio::time::sleep(Duration::from_millis(100)).await;
    world.cluster.state().fail_reads.clear();
    finish_jobs(&world);
    for _ in 0..100 {
        if !provider.holds_lease(&lease_id("lease-sweep")).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!provider.holds_lease(&lease_id("lease-sweep")).await);
    cancel.cancel();
    Ok(())
}

#[tokio::test]
async fn authority_reports_freezes_and_purges_held_leases() -> TestResult {
    let world = World::start().await?;
    let provider = world.restarted(identified(), false).await?;
    let lease = lease_id("lease-authority");
    provider.lease(&lease_request()?, &lease).await?;
    let [entry] = provider
        .authority_held_leases()
        .await?
        .try_into()
        .map_err(|_| "one lease expected")?;
    let HeldLease::Provider { resource_id, .. } = &entry else {
        return Err("expected a provider resource".into());
    };
    let key = resource_id.as_str().to_string();

    assert_eq!(provider.authority_freeze(&entry).await?, FreezeOutcome::Unsupported);
    assert_eq!(provider.authority_thaw(&entry).await?, ThawOutcome::NoLongerOwned);
    let stale = held("lease-authority", "other")?;
    assert_eq!(provider.authority_freeze(&stale).await?, FreezeOutcome::Stale);
    assert_eq!(provider.authority_thaw(&stale).await?, ThawOutcome::Stale);
    let targeted = HeldLease::Targeted(serde_json::from_value::<LeaseAttemptIdentity>(json!({
        "lease_id": "lease-authority", "provider": "k8s_micro_vm",
        "target": {"worker_id": "worker-0", "attempt_id": "attempt-1"},
    }))?);
    assert_eq!(provider.authority_freeze(&targeted).await?, FreezeOutcome::Stale);
    assert_eq!(
        provider.authority_purge(&targeted, || async {}).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(
        provider.authority_purge(&stale, || async {}).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(
        provider
            .authority_purge(&held("lease-unknown", &key)?, || async {})
            .await?,
        PurgeOutcome::Stale
    );

    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = finished.clone();
    let purged = provider
        .authority_purge(&entry, || async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst)
        })
        .await?;
    assert_eq!(purged, PurgeOutcome::Purged);
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!provider.holds_lease(&lease).await);
    assert!(world.cluster.names("jobs").is_empty());
    Ok(())
}

#[tokio::test]
async fn authority_purges_leases_kept_for_cleanup() -> TestResult {
    let world = World::start().await?;
    let provider = world.restarted(identified(), false).await?;
    let lease = lease_id("lease-pending");
    {
        let mut state = world.cluster.state();
        state.fail_creates.push("jobs".into());
        state.fail_deletes = 100;
    }
    let artifact = seed_snapshot(&world.registry, "snapshots/p", b"rootfs").await?;
    let request =
        lease_request_with(json!({"image": {"kind": "snapshot", "reference": serde_json::to_value(&artifact)?}}))?;
    assert!(provider.lease(&request, &lease).await.is_err());
    let [entry] = provider
        .authority_held_leases()
        .await?
        .try_into()
        .map_err(|_| "one lease expected")?;
    assert_eq!(provider.authority_freeze(&entry).await?, FreezeOutcome::Unsupported);

    {
        let mut state = world.cluster.state();
        state.fail_creates.clear();
        state.fail_deletes = 0;
    }
    assert_eq!(
        provider.authority_purge(&entry, || async {}).await?,
        PurgeOutcome::Purged
    );
    assert!(provider.authority_held_leases().await?.is_empty());
    Ok(())
}
