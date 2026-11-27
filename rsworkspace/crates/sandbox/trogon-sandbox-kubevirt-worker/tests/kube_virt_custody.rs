mod fake_cluster;
mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;
use trogon_sandbox_contract::Digest;
use trogon_sandbox_kubevirt_worker::{
    CLAIM_ANNOTATION, CustodyPhase, CustodyToken, DIGEST_ANNOTATION, KubeVirtOps, KubeVirtOpsError, LEASE_LABEL,
    LeaseResources, ROOT_DV_ANNOTATION, ROOT_DV_UID_ANNOTATION, WARM_POOL_LABEL, WARM_POOL_SLOT_LABEL, WarmCandidate,
    WarmClaimIntent,
};

use fake_cluster::{Effect, FakeCluster, Fault, Reactions, Verb};
use support::{
    Boxed, TestResult, digest, lease_journal, metadata_map, ops, receipt, seed_warm_vm, text, touch_vm, uid,
};

async fn candidate(ops: &KubeVirtOps, digest: &Digest) -> Boxed<WarmCandidate> {
    Ok(ops
        .find_confirmed_warm_vm(digest, 512)
        .await?
        .ok_or("no warm candidate")?)
}

fn lease_vm(ops: &KubeVirtOps, lease: &str) -> kube::core::DynamicObject {
    let cloud_init = ops.build_cloud_init_user_data(&[]);
    let labels = ops.lease_labels(lease, "tenant-1");
    ops.build_virtual_machine(&ops.vm_name(lease), &ops.dv_name(lease), 512, None, &cloud_init, labels)
}

fn blank_dv(ops: &KubeVirtOps, name: &str) -> kube::core::DynamicObject {
    ops.build_data_volume_spec(name, json!({"blank": {}}), BTreeMap::new())
}

/// Marks the stored VM `name` claimed with `token` and relabelled to `labels`, as a claim whose reply was lost.
fn mark_claimed(cluster: &FakeCluster, name: &str, token: &CustodyToken, labels: &BTreeMap<String, String>) {
    cluster.update("virtualmachines", name, |vm| {
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.insert(CLAIM_ANNOTATION.to_owned(), token.to_string().into());
        }
        if let Some(existing) = metadata_map(vm, "labels") {
            existing.remove(WARM_POOL_LABEL);
            for (key, value) in labels {
                existing.insert(key.clone(), value.as_str().into());
            }
        }
    });
}

fn pending(candidate: WarmCandidate, token: CustodyToken, labels: BTreeMap<String, String>) -> LeaseResources {
    LeaseResources {
        pending_claim: Some(WarmClaimIntent {
            candidate,
            token,
            labels,
        }),
        ..LeaseResources::default()
    }
}

#[tokio::test]
async fn the_receipt_log_needs_a_receipt_collector() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    assert!(matches!(ops.receipt_log(), Err(KubeVirtOpsError::UnscopedReceipts)));
    ops.persist_resources()?;
    let scoped = ops.with_resource_receipts();
    let log = scoped.receipt_log()?;
    scoped.create_data_volume(blank_dv(&scoped, "dv-1")).await?;
    let logged = log.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(logged, scoped.resource_receipts());
    assert!(logged.data_volume.is_some());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn durable_receipts_follow_every_create_and_the_release() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let ops = ops(&cluster)?;
    let journal = lease_journal(directory.path(), ops.config())?;
    let durable = ops.with_durable_receipts(Arc::clone(&journal), LeaseResources::default());
    let dv_name = durable.dv_name("lease-1");
    durable.create_data_volume(blank_dv(&durable, &dv_name)).await?;
    durable
        .create_virtual_machine_owned(lease_vm(&durable, "lease-1"))
        .await?;
    let record = journal.record()?;
    assert!(record.create.is_none());
    assert_eq!(record.resources, durable.resource_receipts());
    let stored = cluster.get("datavolumes", &dv_name).ok_or("no DataVolume")?;
    assert_eq!(record.resources.data_volume, Some(receipt(&cluster, &stored)?));
    durable.release_resources(&durable.resource_receipts()).await?;
    let record = journal.record()?;
    assert_eq!(record.phase, CustodyPhase::Cleanup);
    assert!(record.resources.is_empty());
    assert!(cluster.is_empty());
    journal.finish()?;
    Ok(())
}

#[tokio::test]
async fn a_create_whose_outcome_is_unknown_blocks_the_release() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let ops = ops(&cluster)?;
    let journal = lease_journal(directory.path(), ops.config())?;
    let durable = ops.with_durable_receipts(Arc::clone(&journal), LeaseResources::default());
    cluster.inject(Fault::new(Verb::Create, "datavolumes", Effect::Disconnect));
    let result = durable.create_data_volume(blank_dv(&durable, "dv-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    assert!(journal.record()?.create.is_some());
    let result = durable.release_resources(&LeaseResources::default()).await;
    assert!(matches!(result, Err(KubeVirtOpsError::OwnershipUnresolved)));
    let result = durable
        .create_virtual_machine_owned(lease_vm(&durable, "lease-1"))
        .await;
    assert!(matches!(result, Err(KubeVirtOpsError::Custody(_))));
    journal.set_phase(CustodyPhase::Unresolved)?;
    let result = durable.release_resources(&LeaseResources::default()).await;
    assert!(matches!(result, Err(KubeVirtOpsError::OwnershipUnresolved)));
    Ok(())
}

#[tokio::test]
async fn the_held_vm_must_still_be_the_receipted_one() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let seed = seed_warm_vm(&cluster, "warm-a", &digest('a')?, None)?;
    let held = receipt(&cluster, &seed.vm)?;
    let current = ops.ensure_current_vm(&held).await?;
    assert_eq!(current.metadata.uid.as_deref(), Some(held.uid.as_str()));
    cluster.recreate("virtualmachines", "warm-a");
    assert!(matches!(
        ops.ensure_current_vm(&held).await,
        Err(KubeVirtOpsError::HeldVmReplaced)
    ));
    cluster.remove("virtualmachines", "warm-a");
    assert!(matches!(
        ops.ensure_current_vm(&held).await,
        Err(KubeVirtOpsError::HeldVmAbsent)
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn releasing_deletes_the_vm_then_the_data_volume() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let dv_name = ops.dv_name("lease-1");
    ops.create_data_volume(blank_dv(&ops, &dv_name)).await?;
    ops.create_virtual_machine_owned(lease_vm(&ops, "lease-1")).await?;
    let owned = ops.resource_receipts();

    cluster.inject(Fault::new(Verb::Delete, "virtualmachines", Effect::Reject(500)));
    let result = ops.release_resources(&owned).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    assert_eq!(ops.resource_receipts(), owned);

    cluster.inject(Fault::new(Verb::Delete, "datavolumes", Effect::Reject(500)));
    let result = ops.release_resources(&owned).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    let left = ops.resource_receipts();
    assert!(left.vm.is_none());
    assert_eq!(left.data_volume, owned.data_volume);
    assert!(cluster.get("virtualmachines", &ops.vm_name("lease-1")).is_none());

    ops.release_resources(&left).await?;
    assert!(ops.resource_receipts().is_empty());
    assert!(cluster.is_empty());
    Ok(())
}

#[tokio::test]
async fn releasing_a_claim_that_took_effect_deletes_the_warm_vm() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let token = CustodyToken::generate();
    let labels = ops.lease_labels("lease-1", "tenant-1");
    mark_claimed(&cluster, "warm-a", &token, &labels);
    ops.release_resources(&pending(candidate, token, labels)).await?;
    assert!(cluster.is_empty());
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn releasing_a_claim_another_writer_beat_leaves_the_warm_vm() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    touch_vm(&cluster, "warm-a");
    let labels = ops.lease_labels("lease-1", "tenant-1");
    ops.release_resources(&pending(candidate, CustodyToken::generate(), labels))
        .await?;
    assert!(cluster.get("virtualmachines", "warm-a").is_some());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn releasing_a_claim_on_a_vm_that_is_gone_with_its_disk_is_settled() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let labels = ops.lease_labels("lease-1", "tenant-1");
    cluster.recreate("virtualmachines", "warm-a");
    cluster.recreate("datavolumes", "warm-a-disk");
    let intent = pending(candidate.clone(), CustodyToken::generate(), labels.clone());
    ops.release_resources(&intent).await?;
    assert!(ops.resource_receipts().is_empty());
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    cluster.delete("virtualmachines", "warm-a");
    cluster.delete("datavolumes", "warm-a-disk");
    ops.release_resources(&intent).await?;
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claim_whose_outcome_cannot_be_told_is_kept_pending() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let token = CustodyToken::generate();
    let labels = ops.lease_labels("lease-1", "tenant-1");

    let unchanged = pending(candidate.clone(), token, labels.clone());
    let result = ops.release_resources(&unchanged).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ClaimUnknownAtRevision)));
    assert_eq!(ops.resource_receipts(), unchanged);

    let other = ops.lease_labels("lease-2", "tenant-1");
    mark_claimed(&cluster, "warm-a", &token, &other);
    let result = ops.release_resources(&unchanged).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ClaimLabelConflict)));

    cluster.remove("virtualmachines", "warm-a");
    let result = ops.release_resources(&unchanged).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ClaimUnknownWithDisk)));
    assert_eq!(cluster.count_requests(Verb::Delete, "datavolumes"), 0);
    Ok(())
}

#[tokio::test]
async fn reconciling_a_pending_claim_keeps_only_a_claim_that_took_effect() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let token = CustodyToken::generate();
    let labels = ops.lease_labels("lease-1", "tenant-1");
    let intent = pending(candidate.clone(), token, labels.clone());

    mark_claimed(&cluster, "warm-a", &token, &labels);
    assert_eq!(
        ops.reconcile_receipts(&intent).await?,
        LeaseResources::claiming(&candidate)
    );

    let lost = pending(candidate, CustodyToken::generate(), labels);
    assert_eq!(ops.reconcile_receipts(&lost).await?, LeaseResources::default());
    Ok(())
}

#[tokio::test]
async fn reconciling_drops_receipts_of_resources_that_are_gone() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let dv_name = ops.dv_name("lease-1");
    ops.create_data_volume(blank_dv(&ops, &dv_name)).await?;
    ops.create_virtual_machine_owned(lease_vm(&ops, "lease-1")).await?;
    let owned = ops.resource_receipts();
    assert_eq!(ops.reconcile_receipts(&owned).await?, owned);
    assert_eq!(
        ops.reconcile_receipts(&LeaseResources::default()).await?,
        LeaseResources::default()
    );

    cluster.recreate("virtualmachines", &ops.vm_name("lease-1"));
    let result = ops.reconcile_receipts(&owned).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ReplacementShared)));

    let only_vm = LeaseResources {
        data_volume: None,
        ..owned.clone()
    };
    assert_eq!(ops.reconcile_receipts(&only_vm).await?, LeaseResources::default());

    cluster.delete("virtualmachines", &ops.vm_name("lease-1"));
    cluster.delete("datavolumes", &dv_name);
    assert_eq!(ops.reconcile_receipts(&owned).await?, LeaseResources::default());
    Ok(())
}

#[tokio::test]
async fn only_idle_warm_vms_with_their_original_disk_are_confirmed() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    for name in [
        "warm-a", "warm-b", "warm-c", "warm-d", "warm-e", "warm-f", "warm-g", "warm-h",
    ] {
        seed_warm_vm(&cluster, name, &digest, None)?;
    }
    cluster.recreate("datavolumes", "warm-b-disk");
    cluster.remove("datavolumes", "warm-c-disk");
    cluster.update("virtualmachines", "warm-d", |vm| {
        if let Some(metadata) = vm.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.insert(
                "finalizers".to_owned(),
                json!(["kubevirt.io/virtualMachineControllerFinalize"]),
            );
            metadata.insert("deletionTimestamp".to_owned(), json!("2026-01-01T00:00:00Z"));
        }
    });
    let drop_annotation = |name: &str, key: &'static str| {
        cluster.update("virtualmachines", name, |vm| {
            if let Some(annotations) = metadata_map(vm, "annotations") {
                annotations.remove(key);
            }
        });
    };
    drop_annotation("warm-e", DIGEST_ANNOTATION);
    drop_annotation("warm-g", ROOT_DV_ANNOTATION);
    drop_annotation("warm-h", ROOT_DV_UID_ANNOTATION);
    cluster.update("virtualmachines", "warm-f", |vm| {
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.insert(DIGEST_ANNOTATION.to_owned(), "not-a-digest".into());
        }
    });
    let confirmed = ops.confirmed_idle_warm_vms().await?;
    let names: Vec<&str> = confirmed.iter().map(|candidate| candidate.vm.name.as_str()).collect();
    assert_eq!(names, ["warm-a"]);
    let only = confirmed.first().ok_or("no candidate")?;
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    let disk = cluster.get("datavolumes", "warm-a-disk").ok_or("no DataVolume")?;
    assert_eq!(only.vm, receipt(&cluster, &vm)?);
    assert_eq!(only.data_volume, receipt(&cluster, &disk)?);
    assert_eq!(
        Some(only.resource_version.as_str()),
        text(&vm, "/metadata/resourceVersion")
    );
    assert_eq!(only.digest, digest);
    cluster.inject(Fault::new(Verb::List, "virtualmachines", Effect::Reject(500)));
    assert!(matches!(
        ops.confirmed_idle_warm_vms().await,
        Err(KubeVirtOpsError::Kube(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_warm_vm_is_found_by_digest_and_memory() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let sized = digest('a')?;
    let legacy = digest('b')?;
    seed_warm_vm(&cluster, "warm-a", &sized, Some(1024))?;
    seed_warm_vm(&cluster, "warm-b", &legacy, None)?;
    seed_warm_vm(&cluster, "warm-c", &digest('c')?, None)?;
    cluster.update("virtualmachines", "warm-c", |vm| {
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.insert(
                trogon_sandbox_kubevirt_worker::WARM_MEMORY_ANNOTATION.to_owned(),
                "lots".into(),
            );
        }
    });
    assert!(ops.find_confirmed_warm_vm(&sized, 2048).await?.is_none());
    let found = ops.find_confirmed_warm_vm(&sized, 1024).await?.ok_or("no sized VM")?;
    assert_eq!(found.vm.name, "warm-a");
    let found = ops.find_confirmed_warm_vm(&legacy, 512).await?.ok_or("no legacy VM")?;
    assert_eq!(found.vm.name, "warm-b");
    assert!(ops.find_confirmed_warm_vm(&legacy, 513).await?.is_none());
    assert!(ops.find_confirmed_warm_vm(&digest('c')?, 1).await?.is_none());
    assert!(ops.find_confirmed_warm_vm(&digest('d')?, 1).await?.is_none());

    let replace = Effect::before(|store| {
        store.recreate("virtualmachines", "warm-a");
    });
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", replace));
    let result = ops.find_confirmed_warm_vm(&sized, 1024).await;
    assert!(matches!(result, Err(KubeVirtOpsError::HeldVmReplaced)));
    Ok(())
}

#[tokio::test]
async fn claiming_needs_a_receipt_collector() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let result = ops
        .claim_confirmed_warm_vm(candidate, ops.lease_labels("lease-1", "tenant-1"))
        .await;
    assert!(matches!(result, Err(KubeVirtOpsError::NoReceiptCollector)));
    assert_eq!(cluster.count_requests(Verb::Patch, "virtualmachines"), 0);
    Ok(())
}

#[tokio::test]
async fn a_claim_relabels_the_vm_its_template_and_its_instance() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let claimed = ops
        .claim_confirmed_warm_vm(candidate.clone(), ops.lease_labels("lease-1", "tenant-1"))
        .await?;
    assert!(claimed);
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    for labels in ["/metadata/labels", "/spec/template/metadata/labels"] {
        let labels = vm.pointer(labels).ok_or("no labels")?;
        assert_eq!(labels.get(LEASE_LABEL), Some(&json!("lease-1")));
        assert!(labels.get(WARM_POOL_LABEL).is_none());
        assert!(labels.get(WARM_POOL_SLOT_LABEL).is_none());
    }
    let token = vm
        .pointer("/metadata/annotations")
        .and_then(|annotations| annotations.get(CLAIM_ANNOTATION))
        .and_then(Value::as_str)
        .ok_or("no claim token")?;
    CustodyToken::new(token)?;
    let vmi = cluster.get("virtualmachineinstances", "warm-a").ok_or("no instance")?;
    let vmi_labels = vmi.pointer("/metadata/labels").ok_or("no instance labels")?;
    assert_eq!(vmi_labels.get(LEASE_LABEL), Some(&json!("lease-1")));
    assert!(vmi_labels.get(WARM_POOL_LABEL).is_none());
    let receipts = ops.resource_receipts();
    assert_eq!(receipts.vm, Some(candidate.vm.clone()));
    assert_eq!(receipts.data_volume, Some(candidate.data_volume.clone()));
    assert!(receipts.pending_claim.is_none());
    let patch = cluster
        .requests()
        .into_iter()
        .find(|request| request.verb == Verb::Patch && request.plural == "virtualmachines")
        .ok_or("no claim patch")?;
    assert_eq!(text(&patch.body, "/metadata/uid"), Some(candidate.vm.uid.as_str()));
    assert_eq!(
        text(&patch.body, "/metadata/resourceVersion"),
        Some(candidate.resource_version.as_str())
    );
    Ok(())
}

#[tokio::test]
async fn a_durable_claim_is_journalled() -> TestResult {
    let cluster = FakeCluster::new();
    let directory = tempfile::tempdir()?;
    let plain = ops(&cluster)?;
    let journal = lease_journal(directory.path(), plain.config())?;
    let ops = plain.with_durable_receipts(Arc::clone(&journal), LeaseResources::default());
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    assert!(
        ops.claim_confirmed_warm_vm(candidate.clone(), ops.lease_labels("lease-1", "tenant-1"))
            .await?
    );
    assert_eq!(journal.record()?.resources, LeaseResources::claiming(&candidate));
    Ok(())
}

#[tokio::test]
async fn a_claim_reply_without_the_token_is_refused() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let strip = Effect::rewrite(|vm| {
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.remove(CLAIM_ANNOTATION);
        }
    });
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", strip));
    let result = ops
        .claim_confirmed_warm_vm(candidate, ops.lease_labels("lease-1", "tenant-1"))
        .await;
    assert!(matches!(result, Err(KubeVirtOpsError::ClaimUnconfirmed)));
    assert!(ops.resource_receipts().pending_claim.is_some());
    Ok(())
}

#[tokio::test]
async fn a_claim_whose_reply_was_lost_is_read_back_as_owned() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", Effect::ApplyThenReject(503)));
    let claimed = ops
        .claim_confirmed_warm_vm(candidate.clone(), ops.lease_labels("lease-1", "tenant-1"))
        .await?;
    assert!(claimed);
    assert_eq!(ops.resource_receipts(), LeaseResources::claiming(&candidate));
    Ok(())
}

#[tokio::test]
async fn an_uncertain_claim_keeps_its_intent_and_deletes_nothing() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", Effect::ApplyThenReject(503)));
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(503)).always());
    let result = ops
        .claim_confirmed_warm_vm(candidate, ops.lease_labels("lease-1", "tenant-1"))
        .await;
    let Err(KubeVirtOpsError::ClaimPending { inspect, .. }) = result else {
        return Err(format!("expected a pending claim, got {result:?}").into());
    };
    assert!(matches!(*inspect, KubeVirtOpsError::Kube(_)));
    let pending = ops.resource_receipts();
    assert!(pending.pending_claim.is_some());
    let deletes = cluster
        .requests()
        .iter()
        .filter(|request| request.verb == Verb::Delete)
        .count();
    assert_eq!(deletes, 0);
    cluster.clear_faults();
    ops.release_resources(&pending).await?;
    assert!(cluster.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claim_beaten_by_a_concurrent_writer_owns_nothing() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let writer = Effect::before(|store| {
        store.update("virtualmachines", "warm-a", |_| {});
    });
    cluster.inject(Fault::new(Verb::Patch, "virtualmachines", writer));
    let claimed = ops
        .claim_confirmed_warm_vm(candidate, ops.lease_labels("lease-1", "tenant-1"))
        .await?;
    assert!(!claimed);
    assert!(ops.resource_receipts().is_empty());
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    assert!(
        vm.pointer("/metadata/annotations")
            .and_then(|annotations| annotations.get(CLAIM_ANNOTATION))
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn a_claimed_vm_without_an_instance_is_still_claimed() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    assert!(
        ops.claim_confirmed_warm_vm(candidate, ops.lease_labels("lease-1", "tenant-1"))
            .await?
    );
    assert_eq!(cluster.count_requests(Verb::Patch, "virtualmachineinstances"), 0);
    Ok(())
}

#[tokio::test]
async fn an_instance_the_claimed_vm_does_not_own_is_not_relabelled() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    cluster.update("virtualmachineinstances", "warm-a", |vmi| {
        if let Some(metadata) = vmi.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.insert(
                "ownerReferences".to_owned(),
                json!([{
                    "apiVersion": "kubevirt.io/v1", "kind": "VirtualMachine", "name": "warm-a", "uid": "someone-else",
                }]),
            );
        }
    });
    let result = ops
        .claim_confirmed_warm_vm(candidate.clone(), ops.lease_labels("lease-1", "tenant-1"))
        .await;
    assert!(matches!(result, Err(KubeVirtOpsError::VmiRelabelForeign)));
    assert_eq!(ops.resource_receipts(), LeaseResources::claiming(&candidate));
    Ok(())
}

#[tokio::test]
async fn an_instance_without_a_revision_is_not_relabelled() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let strip = Effect::rewrite(|vmi| {
        if let Some(metadata) = vmi.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.remove("resourceVersion");
        }
    });
    cluster.inject(Fault::new(Verb::Get, "virtualmachineinstances", strip));
    let result = ops
        .claim_confirmed_warm_vm(candidate, ops.lease_labels("lease-1", "tenant-1"))
        .await;
    assert!(matches!(result, Err(KubeVirtOpsError::VmiRelabelNoRevision)));
    Ok(())
}

#[tokio::test]
async fn the_same_revision_is_claimed_once_and_then_never_evicted() -> TestResult {
    let cluster = FakeCluster::new();
    let base = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&base, &digest).await?;
    let first = base.with_resource_receipts();
    let second = base.with_resource_receipts();
    let (one, two) = tokio::join!(
        first.claim_confirmed_warm_vm(candidate.clone(), first.lease_labels("lease-1", "tenant-1")),
        second.claim_confirmed_warm_vm(candidate.clone(), second.lease_labels("lease-2", "tenant-2")),
    );
    let (one, two) = (one?, two?);
    assert!(one != two);
    let winner = if one { &first } else { &second };
    let loser = if one { &second } else { &first };
    assert_eq!(winner.resource_receipts(), LeaseResources::claiming(&candidate));
    assert!(loser.resource_receipts().is_empty());

    let evictor = base.with_resource_receipts();
    assert!(!evictor.evict_confirmed_idle_vm(&candidate).await?);
    assert!(evictor.resource_receipts().is_empty());
    let deletes = cluster
        .requests()
        .iter()
        .filter(|request| request.verb == Verb::Delete)
        .count();
    assert_eq!(deletes, 0);

    winner.release_resources(&winner.resource_receipts()).await?;
    assert!(cluster.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_claim_keeps_its_intent_for_the_release() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = Arc::new(ops(&cluster)?.with_resource_receipts());
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let release = Arc::new(Notify::new());
    cluster.inject(Fault::new(
        Verb::Patch,
        "virtualmachines",
        Effect::ApplyThenHold(Arc::clone(&release)),
    ));
    let claiming = Arc::clone(&ops);
    let task = tokio::spawn(async move {
        let labels = claiming.lease_labels("lease-1", "tenant-1");
        claiming.claim_confirmed_warm_vm(candidate, labels).await
    });
    let sent = cluster.wait_for_requests(Verb::Patch, "virtualmachines", 1);
    tokio::time::timeout(Duration::from_secs(5), sent).await?;
    task.abort();
    let joined = task.await;
    assert!(joined.is_err_and(|error| error.is_cancelled()));
    release.notify_one();
    let pending = ops.resource_receipts();
    assert!(pending.pending_claim.is_some());
    ops.release_resources(&pending).await?;
    assert!(cluster.is_empty());
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_idle_warm_vm_is_evicted_with_its_disk() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    assert!(ops.evict_confirmed_idle_vm(&candidate).await?);
    assert!(cluster.is_empty());

    let scoped = ops.with_resource_receipts();
    seed_warm_vm(&cluster, "warm-b", &digest, None)?;
    let candidate = self::candidate(&scoped, &digest).await?;
    assert!(scoped.evict_confirmed_idle_vm(&candidate).await?);
    assert_eq!(
        scoped.resource_receipts().data_volume,
        Some(candidate.data_volume.clone())
    );
    let delete = cluster
        .requests()
        .into_iter()
        .rfind(|request| request.verb == Verb::Delete && request.plural == "virtualmachines")
        .ok_or("no delete")?;
    assert_eq!(
        text(&delete.body, "/preconditions/resourceVersion"),
        Some(candidate.resource_version.as_str())
    );
    Ok(())
}

#[tokio::test]
async fn a_changed_warm_vm_is_not_evicted() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    touch_vm(&cluster, "warm-a");
    assert!(!ops.evict_confirmed_idle_vm(&candidate).await?);
    cluster.update("virtualmachines", "warm-a", |vm| {
        if let Some(labels) = metadata_map(vm, "labels") {
            labels.remove(WARM_POOL_LABEL);
        }
    });
    let current = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    let at_current = WarmCandidate {
        resource_version: text(&current, "/metadata/resourceVersion")
            .ok_or("no revision")?
            .to_owned(),
        ..candidate
    };
    assert!(!ops.evict_confirmed_idle_vm(&at_current).await?);
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    assert_eq!(cluster.count_requests(Verb::Delete, "datavolumes"), 0);
    Ok(())
}

#[tokio::test]
async fn evicting_a_vm_that_is_gone_or_replaced_still_deletes_its_disk() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    seed_warm_vm(&cluster, "warm-b", &digest, None)?;
    let candidates = ops.confirmed_idle_warm_vms().await?;
    let [gone, replaced] = candidates.as_slice() else {
        return Err("expected two candidates".into());
    };
    cluster.delete("virtualmachines", "warm-a");
    assert!(ops.evict_confirmed_idle_vm(gone).await?);
    assert!(cluster.get("datavolumes", "warm-a-disk").is_none());
    let replacement = cluster.recreate("virtualmachines", "warm-b").ok_or("no replacement")?;
    assert!(ops.evict_confirmed_idle_vm(replaced).await?);
    assert!(cluster.get("datavolumes", "warm-b-disk").is_none());
    let current = cluster
        .get("virtualmachines", "warm-b")
        .ok_or("the replacement is gone")?;
    assert_eq!(uid(&current)?, uid(&replacement)?);
    Ok(())
}

#[tokio::test]
async fn a_warm_vm_changed_during_eviction_is_kept() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let candidate = candidate(&ops, &digest).await?;
    let writer = Effect::before(|store| {
        store.update("virtualmachines", "warm-a", |_| {});
    });
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", writer).skip(1));
    let result = ops.evict_confirmed_idle_vm(&candidate).await;
    assert!(matches!(result, Err(KubeVirtOpsError::WarmRevisionChanged)));
    assert!(cluster.get("virtualmachines", "warm-a").is_some());
    assert!(cluster.get("datavolumes", "warm-a-disk").is_some());
    Ok(())
}
