mod fake_cluster;
mod support;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use trogon_sandbox_contract::Digest;
use trogon_sandbox_kubevirt_worker::{
    ALLOCATION_ANNOTATION, CLAIM_ANNOTATION, CustodyToken, DIGEST_ANNOTATION, KubeVirtOps, KubeVirtOpsError,
    LEASE_LABEL, ROOT_DV_ANNOTATION, ROOT_DV_UID_ANNOTATION, WARM_MEMORY_ANNOTATION, WARM_POOL_IDLE, WARM_POOL_LABEL,
    WARM_POOL_SLOT_LABEL, WarmClaim, WarmDiskSource, WarmPoolConfig, WarmPoolManager,
};

use fake_cluster::{Effect, FakeCluster, Fault, Verb};
use support::{
    Boxed, ScriptedDiskSource, TestResult, config, digest, metadata_map, reference, seed_warm_vm, text, touch_vm, uid,
    volume,
};

const IDLE_LIMIT: Duration = Duration::from_secs(1800);

struct Pool {
    manager: Arc<WarmPoolManager>,
    source: Arc<ScriptedDiskSource>,
}

/// A pool over `cluster` that restores root disks through `source`, with a volume store under `root` when given.
fn pool_with(
    cluster: &FakeCluster,
    warm: WarmPoolConfig,
    source: ScriptedDiskSource,
    root: Option<&Path>,
) -> Boxed<Pool> {
    let mut config = config(&cluster.namespace())?;
    config.volume = root.map(volume);
    let ops = Arc::new(KubeVirtOps::new(cluster.client(), config));
    let source = Arc::new(source);
    let disks: Arc<dyn WarmDiskSource> = Arc::<ScriptedDiskSource>::clone(&source);
    let manager = Arc::new(WarmPoolManager::new(warm, ops, disks));
    Ok(Pool { manager, source })
}

fn pool(cluster: &FakeCluster) -> Boxed<Pool> {
    pool_with(cluster, WarmPoolConfig::default(), ScriptedDiskSource::default(), None)
}

fn disabled() -> WarmPoolConfig {
    WarmPoolConfig {
        enabled: false,
        ..WarmPoolConfig::default()
    }
}

fn population(entries: &[(Digest, u32)]) -> HashMap<Digest, u32> {
    entries.iter().cloned().collect()
}

fn label<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object.pointer("/metadata/labels")?.get(key)?.as_str()
}

fn annotation<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object.pointer("/metadata/annotations")?.get(key)?.as_str()
}

fn seed_warm(cluster: &FakeCluster, name: &str, fill: char) -> TestResult {
    seed_warm_vm(cluster, name, &digest(fill)?, None)?;
    Ok(())
}

fn drop_revision() -> Effect {
    Effect::rewrite(|object| {
        if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.remove("resourceVersion");
        }
    })
}

#[tokio::test]
async fn the_pool_reports_its_config_and_population() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    assert_eq!(pool.manager.config(), &WarmPoolConfig::default());
    assert!(pool.manager.population_report().await.is_empty());
    seed_warm(&cluster, "warm-a", 'a')?;
    seed_warm(&cluster, "warm-b", 'a')?;
    seed_warm(&cluster, "warm-c", 'b')?;
    pool.manager.reconcile_from_cluster().await?;
    let expected = population(&[(digest('a')?, 2), (digest('b')?, 1)]);
    assert_eq!(pool.manager.population_report().await, expected);
    Ok(())
}

#[tokio::test]
async fn a_lease_claims_a_tracked_warm_vm() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    let digest = digest('a')?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    let claim = pool.manager.try_claim(&digest, 512, "lease-1", "tenant-1").await?;
    let expected = WarmClaim {
        vm_name: "warm-a".to_owned(),
        dv_name: "warm-a-disk".to_owned(),
    };
    assert_eq!(claim, Some(expected));
    assert!(pool.manager.population_report().await.is_empty());
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    assert_eq!(label(&vm, LEASE_LABEL), Some("lease-1"));
    assert_eq!(label(&vm, WARM_POOL_LABEL), None);
    assert_eq!(pool.manager.try_claim(&digest, 512, "lease-2", "tenant-1").await?, None);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_pool_cannot_serve_is_not_claimed() -> TestResult {
    let cluster = FakeCluster::new();
    seed_warm(&cluster, "warm-a", 'a')?;
    let pool = self::pool(&cluster)?;
    assert_eq!(
        pool.manager
            .try_claim(&digest('a')?, 1024, "lease-1", "tenant-1")
            .await?,
        None
    );
    let off = pool_with(&cluster, disabled(), ScriptedDiskSource::default(), None)?;
    assert_eq!(
        off.manager.try_claim(&digest('a')?, 512, "lease-1", "tenant-1").await?,
        None
    );
    assert!(cluster.requests().is_empty());
    assert_eq!(
        pool.manager
            .try_claim(&digest('b')?, 512, "lease-1", "tenant-1")
            .await?,
        None
    );
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    assert_eq!(label(&vm, WARM_POOL_LABEL), Some(WARM_POOL_IDLE));
    Ok(())
}

#[tokio::test]
async fn a_claim_another_writer_beat_is_not_claimed() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    let writer = Effect::before(|store| {
        store.update("virtualmachines", "warm-a", |_| {});
    });
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", writer));
    assert_eq!(
        pool.manager
            .try_claim(&digest('a')?, 512, "lease-1", "tenant-1")
            .await?,
        None
    );
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));
    Ok(())
}

#[tokio::test]
async fn a_failed_lookup_leaves_nothing_to_release() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    cluster.inject(Fault::new(Verb::List, "virtualmachines", Effect::Reject(500)));
    let result = pool.manager.try_claim(&digest('a')?, 512, "lease-1", "tenant-1").await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    assert_eq!(cluster.count_requests(Verb::Delete, "datavolumes"), 0);
    Ok(())
}

#[tokio::test]
async fn a_claim_whose_reply_was_lost_is_released_at_once() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", Effect::ApplyThenReject(500)));
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500)).skip(1));
    let result = pool.manager.try_claim(&digest('a')?, 512, "lease-1", "tenant-1").await;
    assert!(matches!(result, Err(KubeVirtOpsError::ClaimPending { .. })));
    assert!(cluster.get("virtualmachines", "warm-a").is_none());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_none());
    Ok(())
}

#[tokio::test]
async fn cleanup_a_failed_claim_could_not_finish_is_retried_by_reconcile() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", Effect::ApplyThenReject(500)));
    cluster.inject(
        Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500))
            .skip(1)
            .times(2),
    );
    let result = pool.manager.try_claim(&digest('a')?, 512, "lease-1", "tenant-1").await;
    assert!(matches!(result, Err(KubeVirtOpsError::ClaimPending { .. })));
    assert!(cluster.get("virtualmachines", "warm-a").is_some());

    cluster.inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500)));
    pool.manager.reconcile_from_cluster().await?;
    assert!(cluster.get("virtualmachines", "warm-a").is_some());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());

    pool.manager.reconcile_from_cluster().await?;
    assert!(cluster.get("virtualmachines", "warm-a").is_none());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_none());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_finished_lease_prewarms_a_warm_vm_the_next_lease_claims() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let pool = pool_with(
        &cluster,
        WarmPoolConfig::default(),
        ScriptedDiskSource::default(),
        Some(directory.path()),
    )?;
    let digest = digest('a')?;
    pool.manager.spawn_prewarm_after_lease(reference(&digest)).await?;
    assert_eq!(
        pool.manager.population_report().await,
        population(&[(digest.clone(), 1)])
    );

    let [dv_name] = pool
        .source
        .requested()
        .try_into()
        .map_err(|_| "expected one disk request")?;
    let data_volume = cluster.get("datavolumes", &dv_name).ok_or("no DataVolume")?;
    let vms = cluster.objects("virtualmachines");
    let [vm] = vms.as_slice() else {
        return Err("expected one warm VM".into());
    };
    let slot = label(vm, WARM_POOL_SLOT_LABEL).ok_or("no slot")?;
    let token = slot.strip_prefix("warm-").ok_or("the slot is not a warm slot")?;
    token.parse::<CustodyToken>()?;
    assert_eq!(label(vm, WARM_POOL_LABEL), Some(WARM_POOL_IDLE));
    assert_eq!(label(&data_volume, WARM_POOL_SLOT_LABEL), Some(slot));
    assert_eq!(annotation(&data_volume, ALLOCATION_ANNOTATION), Some(slot));
    assert_eq!(annotation(vm, ALLOCATION_ANNOTATION), Some(slot));
    assert_eq!(annotation(vm, DIGEST_ANNOTATION), Some(digest.as_str()));
    assert_eq!(annotation(vm, ROOT_DV_ANNOTATION), Some(dv_name.as_str()));
    assert_eq!(
        annotation(vm, ROOT_DV_UID_ANNOTATION),
        Some(uid(&data_volume)?.as_str())
    );
    assert_eq!(annotation(vm, WARM_MEMORY_ANNOTATION), Some("512"));

    let vm_name = text(vm, "/metadata/name").ok_or("no VM name")?;
    let claim = pool.manager.try_claim(&digest, 512, "lease-1", "tenant-1").await?;
    let expected = WarmClaim {
        vm_name: vm_name.to_owned(),
        dv_name,
    };
    assert_eq!(claim, Some(expected));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn prewarming_stops_at_the_pool_limits() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let root = Some(directory.path());
    let off = pool_with(&cluster, disabled(), ScriptedDiskSource::default(), root)?;
    off.manager.spawn_prewarm_after_lease(reference(&digest('a')?)).await?;
    assert!(off.source.requested().is_empty());
    assert!(cluster.requests().is_empty());

    let per_digest = pool_with(&cluster, WarmPoolConfig::default(), ScriptedDiskSource::default(), root)?;
    per_digest
        .manager
        .spawn_prewarm_after_lease(reference(&digest('a')?))
        .await?;
    per_digest
        .manager
        .spawn_prewarm_after_lease(reference(&digest('a')?))
        .await?;
    assert_eq!(per_digest.source.requested().len(), 1);

    let small = WarmPoolConfig {
        max_pool_size_per_digest: 2,
        max_total_warm: 1,
        ..WarmPoolConfig::default()
    };
    let total = pool_with(&cluster, small, ScriptedDiskSource::default(), root)?;
    total
        .manager
        .spawn_prewarm_after_lease(reference(&digest('b')?))
        .await?;
    total
        .manager
        .spawn_prewarm_after_lease(reference(&digest('b')?))
        .await?;
    assert_eq!(total.source.requested().len(), 1);
    assert_eq!(
        total.manager.population_report().await,
        population(&[(digest('b')?, 1)])
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn prewarming_needs_a_volume_store_and_a_disk() -> TestResult {
    let cluster = FakeCluster::new();
    let unconfigured = pool(&cluster)?;
    unconfigured
        .manager
        .spawn_prewarm_after_lease(reference(&digest('a')?))
        .await?;
    assert!(unconfigured.source.requested().is_empty());
    assert!(cluster.requests().is_empty());

    let directory = tempfile::tempdir()?;
    let failing = pool_with(
        &cluster,
        WarmPoolConfig::default(),
        ScriptedDiskSource::failing(),
        Some(directory.path()),
    )?;
    failing
        .manager
        .spawn_prewarm_after_lease(reference(&digest('a')?))
        .await?;
    assert_eq!(failing.source.requested().len(), 1);
    assert!(cluster.requests().is_empty());
    assert!(failing.manager.population_report().await.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_prewarm_releases_what_it_created() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let pool = pool_with(
        &cluster,
        WarmPoolConfig::default(),
        ScriptedDiskSource::default(),
        Some(directory.path()),
    )?;
    cluster.inject(Fault::new(Verb::Create, "virtualmachines", drop_revision()));
    pool.manager.spawn_prewarm_after_lease(reference(&digest('a')?)).await?;
    assert_eq!(cluster.count_requests(Verb::Create, "virtualmachines"), 1);
    assert!(cluster.is_empty());
    assert!(pool.manager.population_report().await.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_prewarm_retains_cleanup_it_could_not_finish() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let pool = pool_with(
        &cluster,
        WarmPoolConfig::default(),
        ScriptedDiskSource::default(),
        Some(directory.path()),
    )?;
    cluster.inject(Fault::new(Verb::Create, "virtualmachines", drop_revision()));
    cluster.inject(Fault::new(Verb::Delete, "virtualmachines", Effect::Reject(500)));
    pool.manager.spawn_prewarm_after_lease(reference(&digest('a')?)).await?;
    assert_eq!(cluster.names("virtualmachines").len(), 1);
    assert_eq!(cluster.names("datavolumes").len(), 1);
    assert!(pool.manager.population_report().await.is_empty());
    pool.manager.reconcile_from_cluster().await?;
    assert!(cluster.is_empty());
    assert!(pool.manager.population_report().await.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn idle_warm_vms_are_evicted_once_they_idled_long_enough() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    assert_eq!(pool.manager.evict_idle().await, 0);
    tokio::time::advance(IDLE_LIMIT).await;
    assert_eq!(pool.manager.evict_idle().await, 1);
    assert!(cluster.is_empty());
    assert!(pool.manager.population_report().await.is_empty());

    let off = pool_with(&cluster, disabled(), ScriptedDiskSource::default(), None)?;
    seed_warm(&cluster, "warm-b", 'a')?;
    off.manager.reconcile_from_cluster().await?;
    tokio::time::advance(IDLE_LIMIT).await;
    assert_eq!(off.manager.evict_idle().await, 0);
    assert!(cluster.get("virtualmachines", "warm-b").is_some());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_warm_vm_changed_since_it_was_seen_is_evicted_after_a_reconcile() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    touch_vm(&cluster, "warm-a");
    tokio::time::advance(IDLE_LIMIT).await;
    assert_eq!(pool.manager.evict_idle().await, 0);
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));
    pool.manager.reconcile_from_cluster().await?;
    assert_eq!(pool.manager.evict_idle().await, 1);
    assert!(cluster.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_eviction_keeps_what_it_still_owns() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    tokio::time::advance(IDLE_LIMIT).await;

    cluster.inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500)));
    assert_eq!(pool.manager.evict_idle().await, 0);
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));

    cluster.inject(Fault::new(Verb::Delete, "datavolumes", Effect::Reject(500)));
    assert_eq!(pool.manager.evict_idle().await, 0);
    assert!(pool.manager.population_report().await.is_empty());
    assert!(cluster.get("virtualmachines", "warm-a").is_none());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());
    assert_eq!(pool.manager.evict_idle().await, 0);

    pool.manager.reconcile_from_cluster().await?;
    assert!(cluster.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_tracked_warm_vm_that_is_gone_has_its_disk_released() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    cluster.delete("virtualmachines", "warm-a");
    pool.manager.reconcile_from_cluster().await?;
    assert!(pool.manager.population_report().await.is_empty());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());
    pool.manager.reconcile_from_cluster().await?;
    assert!(cluster.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_replaced_warm_vm_gives_up_its_old_receipts_but_never_the_replacement() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    let replacement = cluster.recreate("virtualmachines", "warm-a").ok_or("no replacement")?;
    pool.manager.reconcile_from_cluster().await?;
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    pool.manager.reconcile_from_cluster().await?;
    let current = cluster
        .get("virtualmachines", "warm-a")
        .ok_or("the replacement is gone")?;
    assert_eq!(uid(&current)?, uid(&replacement)?);
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    assert!(cluster.get("datavolumes", "warm-a-disk").is_none());
    Ok(())
}

#[tokio::test]
async fn a_tracked_warm_vm_claimed_elsewhere_is_dropped_without_cleanup() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    cluster.update("virtualmachines", "warm-a", |vm| {
        if let Some(labels) = metadata_map(vm, "labels") {
            labels.remove(WARM_POOL_LABEL);
        }
    });
    pool.manager.reconcile_from_cluster().await?;
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));

    let token = CustodyToken::generate().to_string();
    cluster.update("virtualmachines", "warm-a", |vm| {
        if let Some(labels) = metadata_map(vm, "labels") {
            labels.insert(LEASE_LABEL.to_owned(), "lease-9".into());
        }
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.insert(CLAIM_ANNOTATION.to_owned(), token.as_str().into());
        }
    });
    pool.manager.reconcile_from_cluster().await?;
    assert!(pool.manager.population_report().await.is_empty());
    pool.manager.reconcile_from_cluster().await?;
    assert!(cluster.get("virtualmachines", "warm-a").is_some());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    assert_eq!(cluster.count_requests(Verb::Delete, "datavolumes"), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_idle_warm_vm_whose_disk_changed_is_tracked_at_its_new_revision() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    cluster.recreate("datavolumes", "warm-a-disk").ok_or("no DataVolume")?;
    touch_vm(&cluster, "warm-a");
    pool.manager.reconcile_from_cluster().await?;
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));
    tokio::time::advance(IDLE_LIMIT).await;
    assert_eq!(pool.manager.evict_idle().await, 1);
    assert!(cluster.get("virtualmachines", "warm-a").is_none());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());
    Ok(())
}

#[tokio::test]
async fn reconcile_failures_are_reported() -> TestResult {
    let cluster = FakeCluster::new();
    let pool = pool(&cluster)?;
    cluster.inject(Fault::new(Verb::List, "virtualmachines", Effect::Reject(500)));
    let result = pool.manager.reconcile_from_cluster().await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));

    seed_warm(&cluster, "warm-a", 'a')?;
    pool.manager.reconcile_from_cluster().await?;
    cluster.remove("datavolumes", "warm-a-disk");
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500)));
    let result = pool.manager.reconcile_from_cluster().await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));

    let anonymous = Effect::rewrite(|vm| {
        if let Some(metadata) = vm.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.remove("uid");
        }
    });
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", anonymous));
    let result = pool.manager.reconcile_from_cluster().await;
    assert!(matches!(result, Err(KubeVirtOpsError::ReceiptUid)));
    assert_eq!(pool.manager.population_report().await, population(&[(digest('a')?, 1)]));
    Ok(())
}
