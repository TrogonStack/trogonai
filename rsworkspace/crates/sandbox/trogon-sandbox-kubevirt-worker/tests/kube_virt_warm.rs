mod fake_cluster;
mod support;

use std::collections::BTreeMap;

use serde_json::json;
use trogon_sandbox_kubevirt_worker::{
    DIGEST_ANNOTATION, KubeVirtOpsError, LEASE_LABEL, ROOT_DV_ANNOTATION, ROOT_DV_UID_ANNOTATION, ResourceReceipt,
    TENANT_LABEL, WARM_MEMORY_ANNOTATION, WARM_POOL_IDLE, WARM_POOL_LABEL, WARM_POOL_SLOT_LABEL, WARM_POOL_TENANT,
    WARM_POOL_UNASSIGNED_LEASE, WORKER_LABEL,
};

use fake_cluster::{Effect, FakeCluster, Fault, Reactions, Verb};
use support::{TestResult, digest, metadata_map, ops, seed_warm_vm};

fn drop_annotation(cluster: &FakeCluster, name: &str, key: &str) {
    cluster.update("virtualmachines", name, |vm| {
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.remove(key);
        }
    });
}

#[tokio::test]
async fn idle_warm_vms_are_labelled_for_the_pool_and_this_worker() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let labels = ops.warm_pool_idle_labels(&digest('a')?);
    let expected = BTreeMap::from([
        (WARM_POOL_LABEL.to_owned(), WARM_POOL_IDLE.to_owned()),
        (TENANT_LABEL.to_owned(), WARM_POOL_TENANT.to_owned()),
        (LEASE_LABEL.to_owned(), WARM_POOL_UNASSIGNED_LEASE.to_owned()),
        (WORKER_LABEL.to_owned(), "worker-1".to_owned()),
    ]);
    assert_eq!(labels, expected);
    Ok(())
}

#[tokio::test]
async fn warm_vms_are_annotated_with_their_digest_disk_and_memory() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    let disk = ResourceReceipt {
        namespace: "sandbox".to_owned(),
        name: "dv-1".to_owned(),
        uid: "uid-1".to_owned(),
    };
    let annotations = ops.warm_pool_annotations(&digest, &disk, 768);
    let expected = BTreeMap::from([
        (DIGEST_ANNOTATION.to_owned(), digest.as_str().to_owned()),
        (ROOT_DV_ANNOTATION.to_owned(), "dv-1".to_owned()),
        (ROOT_DV_UID_ANNOTATION.to_owned(), "uid-1".to_owned()),
        (WARM_MEMORY_ANNOTATION.to_owned(), "768".to_owned()),
    ]);
    assert_eq!(annotations, expected);
    Ok(())
}

#[tokio::test]
async fn annotations_are_merged_into_the_vm() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let digest = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &digest, None)?;
    let extra = BTreeMap::from([("example.com/note".to_owned(), "kept".to_owned())]);
    ops.set_virtual_machine_annotations("warm-a", extra).await?;
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    let annotations = vm.pointer("/metadata/annotations").ok_or("no annotations")?;
    assert_eq!(annotations.get("example.com/note"), Some(&json!("kept")));
    assert_eq!(annotations.get(DIGEST_ANNOTATION), Some(&json!(digest.as_str())));
    let result = ops.set_virtual_machine_annotations("missing", BTreeMap::new()).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}

#[tokio::test]
async fn an_idle_warm_vm_is_found_by_its_labels_and_annotations() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let sized = digest('a')?;
    seed_warm_vm(&cluster, "warm-a", &sized, Some(1024))?;
    seed_warm_vm(&cluster, "warm-b", &digest('b')?, None)?;
    drop_annotation(&cluster, "warm-b", ROOT_DV_ANNOTATION);
    seed_warm_vm(&cluster, "warm-c", &digest('c')?, None)?;
    drop_annotation(&cluster, "warm-c", DIGEST_ANNOTATION);

    let found = ops.find_idle_warm_vmi(&sized, 1024).await?;
    assert_eq!(found, Some(("warm-a".to_owned(), "warm-a-disk".to_owned())));
    assert_eq!(ops.find_idle_warm_vmi(&sized, 2048).await?, None);
    let found = ops.find_idle_warm_vmi(&digest('b')?, 512).await?;
    assert_eq!(found, Some(("warm-b".to_owned(), ops.dv_name("warm-b"))));
    assert_eq!(ops.find_idle_warm_vmi(&digest('c')?, 512).await?, None);

    cluster.inject(Fault::new(Verb::List, "virtualmachines", Effect::Reject(500)));
    let result = ops.find_idle_warm_vmi(&sized, 512).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}

#[tokio::test]
async fn idle_warm_vms_are_listed_with_their_digest_and_disk() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    seed_warm_vm(&cluster, "warm-a", &digest('a')?, None)?;
    seed_warm_vm(&cluster, "warm-b", &digest('b')?, None)?;
    drop_annotation(&cluster, "warm-b", ROOT_DV_ANNOTATION);
    seed_warm_vm(&cluster, "warm-c", &digest('c')?, None)?;
    drop_annotation(&cluster, "warm-c", DIGEST_ANNOTATION);
    let listed = ops.list_idle_warm_vmis().await?;
    let expected = vec![
        ("warm-a".to_owned(), digest('a')?, "warm-a-disk".to_owned()),
        ("warm-b".to_owned(), digest('b')?, ops.dv_name("warm-b")),
    ];
    assert_eq!(listed, expected);

    cluster.update("virtualmachines", "warm-c", |vm| {
        if let Some(annotations) = metadata_map(vm, "annotations") {
            annotations.insert(DIGEST_ANNOTATION.to_owned(), "not-a-digest".into());
        }
    });
    assert!(matches!(
        ops.list_idle_warm_vmis().await,
        Err(KubeVirtOpsError::WarmDigest(_))
    ));
    cluster.inject(Fault::new(Verb::List, "virtualmachines", Effect::Reject(500)));
    assert!(matches!(
        ops.list_idle_warm_vmis().await,
        Err(KubeVirtOpsError::Kube(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_unfenced_claim_relabels_the_vm_its_template_and_its_instance() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    seed_warm_vm(&cluster, "warm-a", &digest('a')?, None)?;
    let mut labels = ops.lease_labels("lease-1", "tenant-1");
    labels.insert(WARM_POOL_LABEL.to_owned(), "claimed".to_owned());
    labels.insert(WARM_POOL_SLOT_LABEL.to_owned(), "slot-x".to_owned());
    ops.claim_warm_vmi("warm-a", labels).await?;
    let vm = cluster.get("virtualmachines", "warm-a").ok_or("no VM")?;
    let vmi = cluster.get("virtualmachineinstances", "warm-a").ok_or("no instance")?;
    for labels in [
        vm.pointer("/metadata/labels"),
        vm.pointer("/spec/template/metadata/labels"),
        vmi.pointer("/metadata/labels"),
    ] {
        let labels = labels.ok_or("no labels")?;
        assert_eq!(labels.get(LEASE_LABEL), Some(&json!("lease-1")));
        assert_eq!(labels.get(TENANT_LABEL), Some(&json!("tenant-1")));
        assert_ne!(labels.get(WARM_POOL_LABEL), Some(&json!("claimed")));
    }
    let patch = cluster
        .requests()
        .into_iter()
        .find(|request| request.verb == Verb::Patch && request.plural == "virtualmachines")
        .ok_or("no patch")?;
    let patched = patch.body.pointer("/metadata/labels").ok_or("no patched labels")?;
    assert!(patched.get(WARM_POOL_LABEL).is_none());
    assert!(patched.get(WARM_POOL_SLOT_LABEL).is_none());
    Ok(())
}

#[tokio::test]
async fn an_unfenced_claim_tolerates_a_missing_or_stuck_instance() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    seed_warm_vm(&cluster, "warm-a", &digest('a')?, None)?;
    cluster.inject(Fault::new(Verb::Patch, "virtualmachineinstances", Effect::Reject(500)));
    ops.claim_warm_vmi("warm-a", ops.lease_labels("lease-1", "tenant-1"))
        .await?;
    assert_eq!(cluster.count_requests(Verb::Patch, "virtualmachineinstances"), 1);

    cluster.set_reactions(Reactions::none());
    seed_warm_vm(&cluster, "warm-b", &digest('a')?, None)?;
    ops.claim_warm_vmi("warm-b", ops.lease_labels("lease-2", "tenant-1"))
        .await?;
    assert_eq!(cluster.count_requests(Verb::Patch, "virtualmachineinstances"), 1);

    let result = ops.claim_warm_vmi("missing", BTreeMap::new()).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}
