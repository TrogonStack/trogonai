mod fake_cluster;
mod support;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use trogon_sandbox_kubevirt_worker::{
    CleanupOutcome, GUEST_POLICY_LABEL, KubeVirtOps, KubeVirtOpsError, LEASE_LABEL, ResourceReceipt, TENANT_LABEL,
    WORKER_LABEL, resource_name,
};

use fake_cluster::{Effect, FakeCluster, Fault, Reactions, Verb};
use support::{TestResult, config, dynamic, ops, receipt, ssh_key, text, uid};

fn lease_vm(ops: &KubeVirtOps, lease: &str) -> kube::core::DynamicObject {
    let cloud_init = ops.build_cloud_init_user_data(&[]);
    let labels = ops.lease_labels(lease, "tenant-1");
    ops.build_virtual_machine(&ops.vm_name(lease), &ops.dv_name(lease), 512, None, &cloud_init, labels)
}

fn blank_dv(ops: &KubeVirtOps, name: &str) -> kube::core::DynamicObject {
    ops.build_data_volume_spec(name, json!({"blank": {}}), BTreeMap::new())
}

fn add_bridge(vm: &mut Value) {
    if let Some(interfaces) = vm
        .pointer_mut("/spec/template/spec/domain/devices/interfaces")
        .and_then(Value::as_array_mut)
    {
        interfaces.push(json!({"name": "lan", "bridge": {}}));
    }
}

fn drop_uid(object: &mut Value) {
    if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove("uid");
    }
}

#[tokio::test]
async fn names_and_labels_follow_the_lease() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    assert_eq!(ops.vm_name("lease-1"), resource_name("vm", "lease-1"));
    assert_eq!(ops.dv_name("lease-1"), resource_name("dv", "lease-1"));
    assert_eq!(ops.snapshot_name("lease-1"), resource_name("snap", "lease-1"));
    assert_eq!(ops.config().namespace, "sandbox");
    assert_eq!(ops.client().default_namespace(), "sandbox");
    let labels = ops.lease_labels("lease-1", "tenant-1");
    assert_eq!(labels.get(LEASE_LABEL).map(String::as_str), Some("lease-1"));
    assert_eq!(labels.get(TENANT_LABEL).map(String::as_str), Some("tenant-1"));
    assert_eq!(labels.get(WORKER_LABEL).map(String::as_str), Some("worker-1"));
    Ok(())
}

#[tokio::test]
async fn a_data_volume_spec_includes_the_storage_class_when_configured() -> TestResult {
    let cluster = FakeCluster::new();
    let mut config = config("sandbox")?;
    config.storage_class = Some("fast-ssd".to_owned());
    let ops = KubeVirtOps::new(cluster.client(), config);
    let labels = BTreeMap::from([(LEASE_LABEL.to_owned(), "lease-1".to_owned())]);
    let data_volume = ops.build_data_volume_spec("dv-1", json!({"blank": {}}), labels.clone());
    assert_eq!(data_volume.metadata.namespace.as_deref(), Some("sandbox"));
    assert_eq!(data_volume.metadata.labels, Some(labels));
    let storage = data_volume.data.pointer("/spec/storage").ok_or("no storage")?;
    assert_eq!(storage.get("storageClassName"), Some(&json!("fast-ssd")));
    assert_eq!(storage.pointer("/resources/requests/storage"), Some(&json!("10Gi")));
    assert_eq!(data_volume.data.pointer("/spec/source"), Some(&json!({"blank": {}})));
    Ok(())
}

#[tokio::test]
async fn a_data_volume_spec_omits_the_storage_class_when_unset() -> TestResult {
    let cluster = FakeCluster::new();
    let mut config = config("sandbox")?;
    config.disk_size_gi = 0;
    let ops = KubeVirtOps::new(cluster.client(), config);
    let data_volume = blank_dv(&ops, "dv-1");
    let storage = data_volume.data.pointer("/spec/storage").ok_or("no storage")?;
    assert!(storage.get("storageClassName").is_none());
    assert_eq!(storage.pointer("/resources/requests/storage"), Some(&json!("1Gi")));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn creating_a_data_volume_returns_what_the_server_stored_and_keeps_its_receipt() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let created = ops.create_data_volume_owned(blank_dv(&ops, "dv-1")).await?;
    let stored = cluster
        .get("datavolumes", "dv-1")
        .ok_or("the DataVolume was not stored")?;
    assert_eq!(created.metadata.uid.as_deref(), Some(uid(&stored)?.as_str()));
    assert_eq!(ops.resource_receipts().data_volume, Some(receipt(&cluster, &stored)?));
    ops.wait_data_volume_ready("dv-1").await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn creating_a_data_volume_without_receipts_only_creates_it() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    assert!(cluster.get("datavolumes", "dv-1").is_some());
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_data_volume_needs_a_name() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let mut data_volume = blank_dv(&ops, "dv-1");
    data_volume.metadata.name = None;
    let result = ops.create_data_volume_owned(data_volume).await;
    assert!(matches!(result, Err(KubeVirtOpsError::MissingDataVolumeName)));
    assert!(cluster.requests().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_taken_data_volume_name_is_reported_as_existing() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    let result = ops.create_data_volume(blank_dv(&ops, "dv-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::DataVolumeExists(_))));
    Ok(())
}

#[tokio::test]
async fn a_failed_data_volume_create_is_a_kubernetes_error() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.inject(Fault::new(Verb::Create, "datavolumes", Effect::Reject(500)));
    let ops = ops(&cluster)?;
    let result = ops.create_data_volume(blank_dv(&ops, "dv-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    assert!(cluster.get("datavolumes", "dv-1").is_none());
    Ok(())
}

#[tokio::test]
async fn a_created_data_volume_without_a_uid_cannot_be_receipted() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.inject(Fault::new(Verb::Create, "datavolumes", Effect::rewrite(drop_uid)));
    let ops = ops(&cluster)?.with_resource_receipts();
    let result = ops.create_data_volume(blank_dv(&ops, "dv-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ReceiptUid)));
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_data_volume_replaced_while_importing_is_refused() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?.with_resource_receipts();
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    cluster.recreate("datavolumes", "dv-1");
    let result = ops.wait_data_volume_ready("dv-1").await;
    assert!(matches!(result, Err(KubeVirtOpsError::DataVolumeChanged)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_import_reports_its_message() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    cluster.set_status(
        "datavolumes",
        "dv-1",
        json!({"phase": "Failed", "message": "quota exceeded"}),
    );
    let result = ops.wait_data_volume_ready("dv-1").await;
    let Err(KubeVirtOpsError::DataVolumeFailed { name, message }) = result else {
        return Err(format!("expected a failed import, got {result:?}").into());
    };
    assert_eq!(name, "dv-1");
    assert_eq!(message, "quota exceeded");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_import_error_without_a_message_gets_a_default_one() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    cluster.set_status("datavolumes", "dv-1", json!({"phase": "Error"}));
    let result = ops.wait_data_volume_ready("dv-1").await;
    let Err(KubeVirtOpsError::DataVolumeFailed { message, .. }) = result else {
        return Err(format!("expected a failed import, got {result:?}").into());
    };
    assert_eq!(message, "datavolume import failed");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_import_that_never_finishes_times_out_with_its_last_phase() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    cluster.set_status("datavolumes", "dv-1", json!({"phase": "ImportInProgress"}));
    let started = tokio::time::Instant::now();
    let result = ops.wait_data_volume_ready("dv-1").await;
    let Err(KubeVirtOpsError::DataVolumeTimeout { name, phase }) = result else {
        return Err(format!("expected a timeout, got {result:?}").into());
    };
    assert_eq!((name.as_str(), phase.as_str()), ("dv-1", "ImportInProgress"));
    assert!(started.elapsed() >= trogon_sandbox_kubevirt_worker::KUBEVIRT_WAIT_TIMEOUT);
    Ok(())
}

#[tokio::test]
async fn cloud_init_user_data_includes_sudo_and_every_ssh_key() -> TestResult {
    let cluster = FakeCluster::new();
    let bootstrap = ssh_key("bootstrap");
    let extra = ssh_key("extra");
    let mut config = config("sandbox")?;
    config.bootstrap_ssh_pubkey = Some(bootstrap.clone());
    let ops = KubeVirtOps::new(cluster.client(), config);
    let cloud_init = ops.build_cloud_init_user_data(std::slice::from_ref(&extra));
    assert!(cloud_init.starts_with("#cloud-config\n"));
    assert!(cloud_init.contains("sudo: ALL=(ALL) NOPASSWD:ALL"));
    assert!(cloud_init.contains(&format!("- {bootstrap}")));
    assert!(cloud_init.contains(&format!("- {extra}")));
    Ok(())
}

#[tokio::test]
async fn cloud_init_user_data_disables_env_reset_for_the_sandbox_user_through_a_sudoers_drop_in() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let cloud_init = ops.build_cloud_init_user_data(&[]);
    assert!(cloud_init.contains("Defaults:sandbox !env_reset"));
    assert!(cloud_init.contains("/etc/sudoers.d/90-sandbox-env"));
    assert!(cloud_init.contains("permissions: '0440'"));
    assert!(cloud_init.contains("ssh_authorized_keys:\n      []"));
    Ok(())
}

#[tokio::test]
async fn the_vmi_template_carries_the_guest_policy_label_but_the_vm_does_not() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm = lease_vm(&ops, "lease-1");
    let template = vm
        .data
        .pointer("/spec/template/metadata/labels")
        .ok_or("no template labels")?;
    assert_eq!(template.get(GUEST_POLICY_LABEL), Some(&json!("true")));
    assert_eq!(template.get(LEASE_LABEL), Some(&json!("lease-1")));
    let labels = vm.metadata.labels.as_ref().ok_or("no labels")?;
    assert!(!labels.contains_key(GUEST_POLICY_LABEL));
    assert_eq!(labels.get(LEASE_LABEL).map(String::as_str), Some("lease-1"));
    assert_eq!(vm.data.pointer("/spec/running"), Some(&json!(true)));
    Ok(())
}

#[tokio::test]
async fn an_unsized_domain_requests_only_memory() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm = lease_vm(&ops, "lease-1");
    let domain = vm.data.pointer("/spec/template/spec/domain").ok_or("no domain")?;
    assert_eq!(domain.get("resources"), Some(&json!({"requests": {"memory": "512Mi"}})));
    assert!(domain.get("cpu").is_none());
    Ok(())
}

#[tokio::test]
async fn a_fractional_cpu_request_rounds_cores_up_and_caps_the_time() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm = ops.build_virtual_machine("vm-1", "dv-1", 1024, Some(1500), "#cloud-config\n", BTreeMap::new());
    let domain = vm.data.pointer("/spec/template/spec/domain").ok_or("no domain")?;
    assert_eq!(domain.pointer("/resources/requests/memory"), Some(&json!("1024Mi")));
    assert_eq!(domain.pointer("/resources/requests/cpu"), Some(&json!("1500m")));
    assert_eq!(domain.pointer("/resources/limits/cpu"), Some(&json!("1500m")));
    assert_eq!(domain.get("cpu"), Some(&json!({"cores": 2})));
    let small = ops.build_virtual_machine("vm-1", "dv-1", 512, Some(0), "#cloud-config\n", BTreeMap::new());
    assert_eq!(
        small.data.pointer("/spec/template/spec/domain/cpu"),
        Some(&json!({"cores": 1}))
    );
    Ok(())
}

#[tokio::test]
async fn the_generated_vm_passes_the_nic_binding_check() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm = lease_vm(&ops, "lease-1");
    ops.validate_virtual_machine(&vm)?;
    let volumes = vm.data.pointer("/spec/template/spec/volumes").ok_or("no volumes")?;
    assert_eq!(
        volumes.pointer("/0/dataVolume/name"),
        Some(&json!(ops.dv_name("lease-1")))
    );
    Ok(())
}

#[tokio::test]
async fn bridge_or_secondary_network_bindings_are_reported() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm = dynamic(&json!({
        "metadata": {"name": "vm-1"},
        "spec": {"template": {"spec": {
            "domain": {"devices": {"interfaces": [
                {"name": "default", "masquerade": {}},
                {"name": "lan", "bridge": {}},
                {"sriov": {}},
            ]}},
            "networks": [
                {"name": "default", "pod": {}},
                {"name": "lan", "multus": {"networkName": "br0"}},
                {"multus": {"networkName": "br1"}},
            ],
        }}},
    }))?;
    let result = ops.validate_virtual_machine(&vm);
    let Err(KubeVirtOpsError::UnsafeNicBinding { name, findings }) = result else {
        return Err(format!("expected unsafe bindings, got {result:?}").into());
    };
    assert_eq!(name, "vm-1");
    assert_eq!(findings.len(), 6);
    assert!(findings.iter().any(|finding| finding.contains("`?`")));
    assert!(findings.iter().any(|finding| finding.contains("`bridge`")));
    assert!(findings.iter().any(|finding| finding.contains("`sriov`")));
    assert!(findings.iter().any(|finding| finding.contains("not the pod network")));
    let empty = dynamic(&json!({"metadata": {}}))?;
    let result = ops.validate_virtual_machine(&empty);
    let Err(KubeVirtOpsError::UnsafeNicBinding { name, findings }) = result else {
        return Err(format!("expected unsafe bindings, got {result:?}").into());
    };
    assert_eq!((name.as_str(), findings.len()), ("", 2));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_created_vm_boots_and_reports_its_address() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm = lease_vm(&ops, "lease-1");
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(vm).await?;
    ops.wait_vmi_ready(&vm_name).await?;
    assert!(ops.vmi_is_running(&vm_name).await?);
    let vmi = cluster.get("virtualmachineinstances", &vm_name).ok_or("no instance")?;
    let address = text(&vmi, "/status/interfaces/0/ipAddress").ok_or("no address")?;
    assert_eq!(ops.vmi_ip(&vm_name).await?, address);
    Ok(())
}

#[tokio::test]
async fn a_vm_needs_a_name() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let mut vm = lease_vm(&ops, "lease-1");
    vm.metadata.name = None;
    let result = ops.create_virtual_machine(vm.clone()).await;
    assert!(matches!(result, Err(KubeVirtOpsError::MissingVirtualMachineName)));
    let result = ops.create_virtual_machine_owned(vm).await;
    assert!(matches!(result, Err(KubeVirtOpsError::MissingVirtualMachineName)));
    assert!(cluster.requests().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_taken_vm_name_is_reported_as_existing() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    let result = ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::VirtualMachineExists(_))));
    Ok(())
}

#[tokio::test]
async fn a_stored_vm_with_an_unsafe_nic_is_deleted_again() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.inject(Fault::new(Verb::Create, "virtualmachines", Effect::rewrite(add_bridge)));
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    let result = ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::UnsafeNicBinding { .. })));
    assert!(cluster.get("virtualmachines", &vm_name).is_none());
    assert!(cluster.get("virtualmachineinstances", &vm_name).is_none());
    assert_eq!(cluster.count_requests(Verb::Delete, "virtualmachines"), 1);
    Ok(())
}

#[tokio::test]
async fn a_stored_vm_without_a_uid_cannot_be_receipted() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.inject(Fault::new(Verb::Create, "virtualmachines", Effect::rewrite(drop_uid)).times(2));
    let ops = ops(&cluster)?;
    let result = ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ReceiptUid)));
    let ops = ops.with_resource_receipts();
    let result = ops.create_virtual_machine_owned(lease_vm(&ops, "lease-2")).await;
    assert!(matches!(result, Err(KubeVirtOpsError::ReceiptUid)));
    assert!(ops.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_owned_vm_create_keeps_its_receipt() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let created = ops.create_virtual_machine_owned(lease_vm(&ops, "lease-1")).await?;
    let vm_name = ops.vm_name("lease-1");
    let stored = cluster.get("virtualmachines", &vm_name).ok_or("no VM")?;
    assert_eq!(ops.resource_receipts().vm, Some(receipt(&cluster, &stored)?));
    assert_eq!(ops.receipt(&vm_name, &created)?, receipt(&cluster, &stored)?);
    Ok(())
}

#[tokio::test]
async fn a_receipt_needs_the_name_namespace_and_uid() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let object = dynamic(&json!({"metadata": {"name": "vm-1", "namespace": "sandbox", "uid": "uid-1"}}))?;
    let expected = ResourceReceipt {
        namespace: "sandbox".to_owned(),
        name: "vm-1".to_owned(),
        uid: "uid-1".to_owned(),
    };
    assert_eq!(ops.receipt("vm-1", &object)?, expected);
    assert!(matches!(
        ops.receipt("vm-2", &object),
        Err(KubeVirtOpsError::ReceiptName)
    ));
    let elsewhere = dynamic(&json!({"metadata": {"name": "vm-1", "namespace": "other", "uid": "uid-1"}}))?;
    assert!(matches!(
        ops.receipt("vm-1", &elsewhere),
        Err(KubeVirtOpsError::ReceiptNamespace)
    ));
    let blank = dynamic(&json!({"metadata": {"name": "vm-1", "namespace": "sandbox", "uid": "  "}}))?;
    assert!(matches!(ops.receipt("vm-1", &blank), Err(KubeVirtOpsError::ReceiptUid)));
    Ok(())
}

#[tokio::test]
async fn looking_up_a_resource_answers_none_when_it_is_missing() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    assert!(ops.virtual_machine("vm-1").await?.is_none());
    assert!(ops.data_volume("dv-1").await?.is_none());
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    assert!(ops.data_volume("dv-1").await?.is_some());
    cluster.inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500)));
    assert!(matches!(
        ops.virtual_machine("vm-1").await,
        Err(KubeVirtOpsError::Kube(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_confirmed_delete_removes_exactly_the_receipted_resource() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    let vm_name = ops.vm_name("lease-1");
    let vm = receipt(&cluster, &cluster.get("virtualmachines", &vm_name).ok_or("no VM")?)?;
    let dv = receipt(&cluster, &cluster.get("datavolumes", "dv-1").ok_or("no DataVolume")?)?;
    assert_eq!(
        ops.delete_virtual_machine_confirmed(&vm).await?,
        CleanupOutcome::Deleted
    );
    assert_eq!(ops.delete_data_volume_confirmed(&dv).await?, CleanupOutcome::Deleted);
    assert!(cluster.is_empty());
    assert_eq!(ops.delete_virtual_machine_confirmed(&vm).await?, CleanupOutcome::Absent);
    let delete = cluster
        .requests()
        .into_iter()
        .find(|request| request.verb == Verb::Delete && request.plural == "virtualmachines")
        .ok_or("no delete")?;
    assert_eq!(text(&delete.body, "/propagationPolicy"), Some("Foreground"));
    assert_eq!(text(&delete.body, "/preconditions/uid"), Some(vm.uid.as_str()));
    Ok(())
}

#[tokio::test]
async fn a_confirmed_delete_refuses_a_receipt_from_elsewhere() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let elsewhere = ResourceReceipt {
        namespace: "other".to_owned(),
        name: "vm-1".to_owned(),
        uid: "uid-1".to_owned(),
    };
    let result = ops.delete_virtual_machine_confirmed(&elsewhere).await;
    assert!(matches!(result, Err(KubeVirtOpsError::CleanupReceipt)));
    let blank = ResourceReceipt {
        namespace: "sandbox".to_owned(),
        name: "vm-1".to_owned(),
        uid: " ".to_owned(),
    };
    let result = ops.delete_virtual_machine_confirmed(&blank).await;
    assert!(matches!(result, Err(KubeVirtOpsError::CleanupReceipt)));
    assert!(cluster.requests().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_confirmed_delete_leaves_a_same_name_replacement_alone() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    let original = receipt(&cluster, &cluster.get("datavolumes", "dv-1").ok_or("no DataVolume")?)?;
    let replacement = cluster.recreate("datavolumes", "dv-1").ok_or("no replacement")?;
    assert_eq!(
        ops.delete_data_volume_confirmed(&original).await?,
        CleanupOutcome::Absent
    );
    assert_eq!(cluster.get("datavolumes", "dv-1"), Some(replacement));
    assert_eq!(cluster.count_requests(Verb::Delete, "datavolumes"), 0);
    Ok(())
}

#[tokio::test]
async fn a_confirmed_delete_settles_every_delete_answer() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    let dv = receipt(&cluster, &cluster.get("datavolumes", "dv-1").ok_or("no DataVolume")?)?;

    cluster.inject(Fault::new(Verb::Delete, "datavolumes", Effect::Reject(404)));
    assert_eq!(ops.delete_data_volume_confirmed(&dv).await?, CleanupOutcome::Absent);

    cluster.inject(Fault::new(Verb::Delete, "datavolumes", Effect::Reject(500)));
    let result = ops.delete_data_volume_confirmed(&dv).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));

    cluster.inject(Fault::new(Verb::Delete, "datavolumes", Effect::Ignore));
    let result = ops.delete_data_volume_confirmed(&dv).await;
    let Err(KubeVirtOpsError::StillPresent { name }) = result else {
        return Err(format!("expected the DataVolume to remain, got {result:?}").into());
    };
    assert_eq!(name, "dv-1");

    let replace = Effect::before(|store| {
        store.recreate("datavolumes", "dv-1");
    });
    cluster.inject(Fault::new(Verb::Delete, "datavolumes", replace));
    assert_eq!(ops.delete_data_volume_confirmed(&dv).await?, CleanupOutcome::Absent);
    assert!(cluster.get("datavolumes", "dv-1").is_some());
    Ok(())
}

#[tokio::test]
async fn a_confirmed_delete_waits_out_finalizers() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    cluster.update("datavolumes", "dv-1", |object| {
        if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.insert("finalizers".to_owned(), json!(["cdi.kubevirt.io/dataVolumeFinalizer"]));
        }
    });
    let dv = receipt(&cluster, &cluster.get("datavolumes", "dv-1").ok_or("no DataVolume")?)?;
    let result = ops.delete_data_volume_confirmed(&dv).await;
    assert!(matches!(result, Err(KubeVirtOpsError::StillPresent { .. })));
    let terminating = cluster.get("datavolumes", "dv-1").ok_or("no DataVolume")?;
    assert!(terminating.pointer("/metadata/deletionTimestamp").is_some());
    cluster.update("datavolumes", "dv-1", |object| {
        if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.remove("finalizers");
        }
    });
    assert!(cluster.get("datavolumes", "dv-1").is_none());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn readiness_with_a_held_vm_receipt_needs_the_instance_to_be_owned_by_it() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine_owned(lease_vm(&ops, "lease-1")).await?;
    ops.wait_vmi_ready(&vm_name).await?;
    cluster.update("virtualmachineinstances", &vm_name, |vmi| {
        if let Some(metadata) = vmi.get_mut("metadata").and_then(Value::as_object_mut) {
            metadata.insert(
                "ownerReferences".to_owned(),
                json!([{
                    "apiVersion": "kubevirt.io/v1", "kind": "VirtualMachine", "name": "other", "uid": "someone-else",
                }]),
            );
        }
    });
    let result = ops.wait_vmi_ready(&vm_name).await;
    assert!(matches!(result, Err(KubeVirtOpsError::VmiOwnerMismatch)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn readiness_with_a_held_vm_receipt_needs_the_vm_to_remain() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?.with_resource_receipts();
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine_owned(lease_vm(&ops, "lease-1")).await?;
    cluster.remove("virtualmachines", &vm_name);
    let result = ops.wait_vmi_ready(&vm_name).await;
    assert!(matches!(result, Err(KubeVirtOpsError::HeldVmAbsent)));

    let ops = ops.with_resource_receipts();
    let vm_name = ops.vm_name("lease-2");
    ops.create_virtual_machine_owned(lease_vm(&ops, "lease-2")).await?;
    cluster.recreate("virtualmachines", &vm_name);
    let result = ops.wait_vmi_ready(&vm_name).await;
    assert!(matches!(result, Err(KubeVirtOpsError::HeldVmReplaced)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_instance_is_reported() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    cluster.set_status("virtualmachineinstances", &vm_name, json!({"phase": "Failed"}));
    let result = ops.wait_vmi_ready(&vm_name).await;
    let Err(KubeVirtOpsError::VmiFailed { name }) = result else {
        return Err(format!("expected a failed instance, got {result:?}").into());
    };
    assert_eq!(name, vm_name);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_instance_that_never_becomes_ready_times_out() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.set_reactions(Reactions::none());
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    let result = ops.wait_vmi_ready(&vm_name).await;
    assert!(matches!(result, Err(KubeVirtOpsError::VmiReadyTimeout { .. })));
    cluster.run_vmi(&vm_name);
    cluster.set_status("virtualmachineinstances", &vm_name, json!({"phase": "Running"}));
    let result = ops.wait_vmi_ready(&vm_name).await;
    assert!(matches!(result, Err(KubeVirtOpsError::VmiReadyTimeout { .. })));
    cluster.run_vmi(&vm_name);
    ops.wait_vmi_ready(&vm_name).await?;
    Ok(())
}

#[tokio::test]
async fn the_first_reported_interface_address_is_used() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    let interfaces = json!({"phase": "Running", "interfaces": [
        {"name": "default"},
        {"name": "x", "ipAddress": ""},
        {"name": "y", "ipAddress": "10.244.1.7"},
        {"name": "z", "ipAddress": "10.244.1.8"},
    ]});
    cluster.set_status("virtualmachineinstances", &vm_name, interfaces);
    assert_eq!(ops.vmi_ip(&vm_name).await?, "10.244.1.7");
    Ok(())
}

#[tokio::test]
async fn no_interfaces_means_no_address() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    for status in [json!({}), json!({"interfaces": []}), Value::Null] {
        cluster.set_status("virtualmachineinstances", &vm_name, status);
        let result = ops.vmi_ip(&vm_name).await;
        assert!(matches!(result, Err(KubeVirtOpsError::NoInterfaceAddress { .. })));
    }
    let result = ops.vmi_ip("missing").await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_vm_stops_and_starts_through_its_run_state() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    assert!(ops.vmi_is_running(&vm_name).await?);
    ops.set_virtual_machine_running(&vm_name, false).await?;
    ops.wait_vmi_stopped(&vm_name).await?;
    assert!(!ops.vmi_is_running(&vm_name).await?);
    let stored = cluster.get("virtualmachines", &vm_name).ok_or("no VM")?;
    assert_eq!(stored.pointer("/spec/running"), Some(&json!(false)));
    ops.set_virtual_machine_running(&vm_name, true).await?;
    ops.wait_vmi_ready(&vm_name).await?;
    cluster.set_status("virtualmachineinstances", &vm_name, json!({"phase": "Succeeded"}));
    assert!(!ops.vmi_is_running(&vm_name).await?);
    ops.wait_vmi_stopped(&vm_name).await?;
    let result = ops.set_virtual_machine_running("missing", true).await;
    assert!(matches!(result, Err(KubeVirtOpsError::Kube(_))));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_instance_that_keeps_running_times_out_stopping() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    cluster.set_reactions(Reactions::none());
    ops.set_virtual_machine_running(&vm_name, false).await?;
    let result = ops.wait_vmi_stopped(&vm_name).await;
    let Err(KubeVirtOpsError::VmiStopTimeout { name }) = result else {
        return Err(format!("expected a stop timeout, got {result:?}").into());
    };
    assert_eq!(name, vm_name);
    Ok(())
}

#[tokio::test]
async fn an_unconfirmed_delete_removes_whatever_has_the_name() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops(&cluster)?;
    let vm_name = ops.vm_name("lease-1");
    ops.create_virtual_machine(lease_vm(&ops, "lease-1")).await?;
    ops.create_data_volume(blank_dv(&ops, "dv-1")).await?;
    ops.delete_virtual_machine(&vm_name).await?;
    ops.delete_data_volume("dv-1").await?;
    assert!(cluster.is_empty());
    ops.delete_virtual_machine(&vm_name).await?;
    ops.delete_data_volume("dv-1").await?;
    cluster.inject(Fault::new(Verb::Delete, "virtualmachines", Effect::Reject(500)));
    assert!(matches!(
        ops.delete_virtual_machine(&vm_name).await,
        Err(KubeVirtOpsError::Kube(_))
    ));
    cluster.inject(Fault::new(Verb::Delete, "datavolumes", Effect::Reject(500)));
    assert!(matches!(
        ops.delete_data_volume("dv-1").await,
        Err(KubeVirtOpsError::Kube(_))
    ));
    Ok(())
}
