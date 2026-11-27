mod fake_cluster;
mod support;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use trogon_sandbox_kubevirt_worker::{
    EXPORT_POD_IMAGE, EXPORT_POD_TIMEOUT, KubeVirtOps, KubeVirtOpsError, LEASE_LABEL, SNAPSHOT_SOURCE_LABEL,
    resource_name,
};

use fake_cluster::{Effect, FakeCluster, Fault, Reactions, Verb};
use support::{Boxed, TestResult, ops, text};

const CONTENTS: &str = "virtualmachinesnapshotcontents";

fn export_pod_name(pvc: &str) -> String {
    resource_name("export", pvc)
}

async fn snapshot(ops: &KubeVirtOps, name: &str) -> Boxed<()> {
    let labels = BTreeMap::from([(LEASE_LABEL.to_owned(), "lease-1".to_owned())]);
    ops.create_vm_snapshot(name, &ops.vm_name("lease-1"), labels).await?;
    Ok(())
}

fn content(name: &str, labels: Value, pvc: Option<&str>) -> Value {
    let status = pvc.map_or_else(
        || json!({"readyToUse": true}),
        |pvc| json!({"readyToUse": true, "volumeBackups": [{"volumeName": "rootdisk", "persistentVolumeClaimName": pvc}]}),
    );
    json!({
        "apiVersion": "snapshot.kubevirt.io/v1beta1",
        "kind": "VirtualMachineSnapshotContent",
        "metadata": {"name": name, "labels": labels},
        "spec": {},
        "status": status,
    })
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_is_taken_exported_and_deleted() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let name = ops.snapshot_name("lease-1");
    snapshot(&ops, &name).await?;
    let stored = cluster.get("virtualmachinesnapshots", &name).ok_or("no snapshot")?;
    assert_eq!(text(&stored, "/spec/source/kind"), Some("VirtualMachine"));
    assert_eq!(text(&stored, "/spec/source/apiGroup"), Some("kubevirt.io"));
    assert_eq!(
        text(&stored, "/spec/source/name"),
        Some(ops.vm_name("lease-1").as_str())
    );
    assert_eq!(
        stored
            .pointer("/metadata/labels")
            .and_then(|labels| labels.get(LEASE_LABEL)),
        Some(&json!("lease-1"))
    );
    ops.wait_vm_snapshot_ready(&name).await?;
    let pvc = ops.snapshot_pvc_name(&name).await?;
    assert_eq!(pvc, format!("{name}-rootdisk"));
    cluster.set_pvc_contents(&pvc, "root disk bytes");
    assert_eq!(ops.read_pvc_bytes(&pvc).await?, b"root disk bytes");
    let pod = cluster
        .requests()
        .into_iter()
        .find(|request| request.verb == Verb::Create && request.plural == "pods")
        .ok_or("no export pod")?;
    assert_eq!(text(&pod.body, "/spec/containers/0/image"), Some(EXPORT_POD_IMAGE));
    assert_eq!(text(&pod.body, "/spec/restartPolicy"), Some("Never"));
    assert_eq!(
        text(&pod.body, "/spec/volumes/0/persistentVolumeClaim/claimName"),
        Some(pvc.as_str())
    );
    assert_eq!(
        pod.body.pointer("/spec/volumes/0/persistentVolumeClaim/readOnly"),
        Some(&json!(true))
    );
    assert!(cluster.names("pods").is_empty());
    ops.delete_vm_snapshot(&name).await?;
    assert!(cluster.is_empty());
    ops.delete_vm_snapshot(&name).await?;
    Ok(())
}

#[tokio::test]
async fn a_snapshot_create_failure_is_reported() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    snapshot(&ops, "snap-1").await?;
    let result = ops
        .create_vm_snapshot("snap-1", &ops.vm_name("lease-1"), BTreeMap::new())
        .await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    cluster.inject(Fault::new(Verb::Delete, "virtualmachinesnapshots", Effect::Reject(500)));
    assert!(matches!(
        ops.delete_vm_snapshot("snap-1").await,
        Err(KubeVirtOpsError::Kube(_))
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_snapshot_stops_the_wait() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    snapshot(&ops, "snap-1").await?;
    let failure = json!({"readyToUse": false, "conditions": [{"type": "Failure", "status": "True"}]});
    cluster.set_status("virtualmachinesnapshots", "snap-1", failure);
    let result = ops.wait_vm_snapshot_ready("snap-1").await;
    let Err(KubeVirtOpsError::SnapshotFailed { name }) = result else {
        return Err(format!("expected a failed snapshot, got {result:?}").into());
    };
    assert_eq!(name, "snap-1");
    let result = ops.wait_vm_snapshot_ready("missing").await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_that_never_becomes_ready_times_out() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    snapshot(&ops, "snap-1").await?;
    let result = ops.wait_vm_snapshot_ready("snap-1").await;
    assert!(matches!(result, Err(KubeVirtOpsError::SnapshotTimeout { .. })));
    cluster.ready_snapshot("snap-1");
    ops.wait_vm_snapshot_ready("snap-1").await?;
    Ok(())
}

#[tokio::test]
async fn the_snapshot_pvc_falls_back_to_contents_named_after_the_snapshot() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let labelled = json!({(SNAPSHOT_SOURCE_LABEL): "snap-1"});
    cluster.insert(CONTENTS, content("content-for-snap-1-empty", labelled, None));
    cluster.insert(CONTENTS, content("unrelated", json!({}), Some("other-pvc")));
    cluster.insert(CONTENTS, content("content-for-snap-1", json!({}), Some("pvc-1")));
    assert_eq!(ops.snapshot_pvc_name("snap-1").await?, "pvc-1");
    Ok(())
}

#[tokio::test]
async fn the_snapshot_pvc_survives_a_failed_labelled_lookup() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    snapshot(&ops, "snap-1").await?;
    cluster.inject(Fault::new(Verb::List, CONTENTS, Effect::Reject(500)));
    assert_eq!(ops.snapshot_pvc_name("snap-1").await?, "snap-1-rootdisk");
    let lists = cluster.count_requests(Verb::List, CONTENTS);
    assert_eq!(lists, 2);
    Ok(())
}

#[tokio::test]
async fn a_snapshot_without_a_backed_up_pvc_is_reported() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    cluster.insert(CONTENTS, content("unrelated", json!({}), Some("other-pvc")));
    let result = ops.snapshot_pvc_name("snap-1").await;
    let Err(KubeVirtOpsError::SnapshotPvcMissing { name }) = result else {
        return Err(format!("expected a missing PVC, got {result:?}").into());
    };
    assert_eq!(name, "snap-1");
    cluster.inject(Fault::new(Verb::List, CONTENTS, Effect::Reject(500)).times(2));
    let result = ops.snapshot_pvc_name("snap-1").await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_export_replaces_a_stale_pod_and_reads_the_logs_of_a_failed_one() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let pod_name = export_pod_name("pvc-1");
    cluster.insert(
        "pods",
        json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": pod_name},
            "spec": {"containers": [{"name": "stale"}]},
            "status": {"phase": "Running"},
        }),
    );
    cluster.set_reactions(Reactions::none());
    let fail = Effect::rewrite(|pod| {
        if let Some(fields) = pod.as_object_mut() {
            fields.insert("status".to_owned(), json!({"phase": "Failed"}));
        }
    });
    cluster.inject(Fault::new(Verb::Get, "pods", fail).skip(1));
    cluster.set_pod_logs(&pod_name, "partial");
    assert_eq!(ops.read_pvc_bytes("pvc-1").await?, b"partial");
    assert_eq!(cluster.count_requests(Verb::Delete, "pods"), 2);
    assert_eq!(cluster.count_requests(Verb::Get, "pods"), 2);
    assert!(cluster.names("pods").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_export_pod_that_never_finishes_is_abandoned() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    let started = tokio::time::Instant::now();
    let result = ops.read_pvc_bytes("pvc-1").await;
    let Err(KubeVirtOpsError::ExportPodTimeout { name }) = result else {
        return Err(format!("expected an export timeout, got {result:?}").into());
    };
    assert_eq!(name, export_pod_name("pvc-1"));
    assert!(started.elapsed() >= EXPORT_POD_TIMEOUT);
    assert!(cluster.names("pods").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_empty_export_is_refused() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let result = ops.read_pvc_bytes("pvc-1").await;
    let Err(KubeVirtOpsError::EmptyExport { pvc }) = result else {
        return Err(format!("expected an empty export, got {result:?}").into());
    };
    assert_eq!(pvc, "pvc-1");
    assert!(cluster.names("pods").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn export_failures_are_kubernetes_errors() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    cluster.set_pvc_contents("pvc-1", "bytes");
    let faults = [
        Fault::new(Verb::Create, "pods", Effect::Reject(500)),
        Fault::new(Verb::Get, "pods", Effect::Reject(500)),
        Fault::new(Verb::Logs, "pods", Effect::Reject(500)),
        Fault::new(Verb::Delete, "pods", Effect::Reject(500)).skip(1),
    ];
    for fault in faults {
        cluster.inject(fault);
        let result = ops.read_pvc_bytes("pvc-1").await;
        assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
        cluster.clear_faults();
    }
    assert_eq!(ops.read_pvc_bytes("pvc-1").await?, b"bytes");
    Ok(())
}
