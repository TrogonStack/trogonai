//! Fixtures shared by the integration tests: a worker configuration, operations over a [`FakeCluster`], warm VMs
//! seeded as another worker would have left them, a custody journal, and a scripted [`WarmDiskSource`].

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use kube::core::DynamicObject;
use serde_json::{Map, Value, json};
use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, OciArtifactRef, TenantId, WorkerId};
use trogon_sandbox_kubevirt_worker::{
    CustodyJournal, DIGEST_ANNOTATION, KubeVirtConfig, KubeVirtOps, LEASE_LABEL, LeaseJournal, ROOT_DV_ANNOTATION,
    ROOT_DV_UID_ANNOTATION, ResourceReceipt, TENANT_LABEL, WARM_MEMORY_ANNOTATION, WARM_POOL_IDLE, WARM_POOL_LABEL,
    WARM_POOL_SLOT_LABEL, WARM_POOL_TENANT, WARM_POOL_UNASSIGNED_LEASE, WORKER_LABEL, WarmDiskSource,
    WarmDiskSourceError, WarmPoolConfig,
};
use trogon_sandbox_oci::OciClientConfig;
use trogon_sandbox_volumes::{VolumeBackend, VolumeClientConfig};

use crate::fake_cluster::FakeCluster;

pub type TestResult = Result<(), Box<dyn Error>>;

pub type Boxed<T> = Result<T, Box<dyn Error>>;

/// The worker every fixture belongs to.
pub fn worker() -> &'static str {
    "worker-1"
}

/// A worker configuration for `namespace` with a 10 GiB disk and the default warm pool.
pub fn config(namespace: &str) -> Boxed<KubeVirtConfig> {
    Ok(KubeVirtConfig {
        namespace: namespace.to_owned(),
        default_image: "registry.example/sandbox/base:latest".to_owned(),
        storage_class: None,
        snapshot_registry: "registry.example".to_owned(),
        worker_id: WorkerId::new(worker())?,
        disk_size_gi: 10,
        bootstrap_ssh_pubkey: None,
        s3_secret_name: None,
        restore_bucket_prefix: "snapshots/".to_owned(),
        oci: OciClientConfig::default(),
        cosign_verifier: None,
        volume: None,
        warm_pool: WarmPoolConfig::default(),
    })
}

/// A volume store on the local file system under `root`.
pub fn volume(root: &Path) -> VolumeClientConfig {
    VolumeClientConfig {
        backend: VolumeBackend::LocalFs {
            root: root.to_path_buf(),
        },
        kms_key_id: None,
    }
}

/// Operations of the default worker in the cluster's namespace.
pub fn ops(cluster: &FakeCluster) -> Boxed<KubeVirtOps> {
    Ok(KubeVirtOps::new(cluster.client(), config(&cluster.namespace())?))
}

/// A digest made of 64 copies of `fill`.
pub fn digest(fill: char) -> Boxed<Digest> {
    Ok(Digest::new(format!("sha256:{}", fill.to_string().repeat(64)))?)
}

/// A KubeVirt disk snapshot artifact with `digest`.
pub fn reference(digest: &Digest) -> OciArtifactRef {
    OciArtifactRef {
        registry: "registry.example".to_owned(),
        repository: "sandbox/snapshots".to_owned(),
        digest: digest.clone(),
        artifact_type: ArtifactType::KubevirtDiskV1,
    }
}

/// An SSH public key line for `comment`, built at run time.
pub fn ssh_key(comment: &str) -> String {
    let material = STANDARD.encode(format!("fixture key material for {comment}"));
    format!("ssh-ed25519 {material} {comment}")
}

/// The string at JSON pointer `pointer` in `object`.
pub fn text<'a>(object: &'a Value, pointer: &str) -> Option<&'a str> {
    object.pointer(pointer).and_then(Value::as_str)
}

/// The uid the cluster gave `object`.
pub fn uid(object: &Value) -> Boxed<String> {
    Ok(text(object, "/metadata/uid").ok_or("the object has no uid")?.to_owned())
}

/// The receipt of a stored `object` in the fixture namespace.
pub fn receipt(cluster: &FakeCluster, object: &Value) -> Boxed<ResourceReceipt> {
    Ok(ResourceReceipt {
        namespace: cluster.namespace(),
        name: text(object, "/metadata/name")
            .ok_or("the object has no name")?
            .to_owned(),
        uid: uid(object)?,
    })
}

/// The `metadata.{field}` map of `object`, such as its labels or annotations, created when missing.
pub fn metadata_map<'a>(object: &'a mut Value, field: &str) -> Option<&'a mut Map<String, Value>> {
    let metadata = object.get_mut("metadata")?.as_object_mut()?;
    metadata
        .entry(field)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
}

/// Bumps the resource version of the stored VM `name` without changing it, as an unrelated writer would.
pub fn touch_vm(cluster: &FakeCluster, name: &str) {
    cluster.update("virtualmachines", name, |_| {});
}

/// A typed copy of a stored object.
pub fn dynamic(object: &Value) -> Boxed<DynamicObject> {
    Ok(serde_json::from_value(object.clone())?)
}

/// The labels of an idle warm VM in slot `slot`.
pub fn idle_labels(slot: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (WARM_POOL_LABEL.to_owned(), WARM_POOL_IDLE.to_owned()),
        (WARM_POOL_SLOT_LABEL.to_owned(), slot.to_owned()),
        (TENANT_LABEL.to_owned(), WARM_POOL_TENANT.to_owned()),
        (LEASE_LABEL.to_owned(), WARM_POOL_UNASSIGNED_LEASE.to_owned()),
        (WORKER_LABEL.to_owned(), worker().to_owned()),
    ])
}

/// A stored idle warm VM and its root DataVolume.
pub struct WarmSeed {
    pub vm: Value,
    pub data_volume: Value,
}

/// Seeds an idle warm VM `name` restored from `digest`, set to run, whose root DataVolume `{name}-disk` already
/// succeeded. `memory_mib` is its warm memory annotation; without one it is a VM from before the annotation.
pub fn seed_warm_vm(cluster: &FakeCluster, name: &str, digest: &Digest, memory_mib: Option<u32>) -> Boxed<WarmSeed> {
    let dv_name = format!("{name}-disk");
    let labels = idle_labels(&format!("slot-{name}"));
    let data_volume = cluster.insert(
        "datavolumes",
        json!({
            "apiVersion": "cdi.kubevirt.io/v1beta1",
            "kind": "DataVolume",
            "metadata": {"name": dv_name, "labels": labels},
            "spec": {"source": {"blank": {}}},
            "status": {"phase": "Succeeded"},
        }),
    );
    let mut annotations = Map::new();
    annotations.insert(DIGEST_ANNOTATION.to_owned(), digest.as_str().into());
    annotations.insert(ROOT_DV_ANNOTATION.to_owned(), dv_name.as_str().into());
    annotations.insert(ROOT_DV_UID_ANNOTATION.to_owned(), uid(&data_volume)?.into());
    if let Some(memory_mib) = memory_mib {
        annotations.insert(WARM_MEMORY_ANNOTATION.to_owned(), memory_mib.to_string().into());
    }
    let vm = cluster.insert(
        "virtualmachines",
        json!({
            "apiVersion": "kubevirt.io/v1",
            "kind": "VirtualMachine",
            "metadata": {"name": name, "labels": labels, "annotations": annotations},
            "spec": {
                "running": true,
                "template": {
                    "metadata": {"labels": labels},
                    "spec": {
                        "domain": {"devices": {"interfaces": [{"name": "default", "masquerade": {}}]}},
                        "networks": [{"name": "default", "pod": {}}],
                        "volumes": [{"name": "rootdisk", "dataVolume": {"name": dv_name}}],
                    },
                },
            },
        }),
    );
    Ok(WarmSeed { vm, data_volume })
}

/// A custody journal in `directory` for the default worker, and the journal of lease `lease-1` in it.
pub fn lease_journal(directory: &Path, config: &KubeVirtConfig) -> Boxed<Arc<LeaseJournal>> {
    let journal = CustodyJournal::open(directory, config.worker_id.clone(), &config.namespace)?;
    Ok(LeaseJournal::begin(
        &journal,
        &LeaseId::new("lease-1"),
        &TenantId::new("tenant-1")?,
    )?)
}

/// A [`WarmDiskSource`] that describes a blank DataVolume, or fails when told to, and records every DataVolume
/// it was asked for.
#[derive(Default)]
pub struct ScriptedDiskSource {
    fail: bool,
    requested: Mutex<Vec<String>>,
}

impl ScriptedDiskSource {
    pub fn failing() -> Self {
        Self {
            fail: true,
            requested: Mutex::default(),
        }
    }

    /// The DataVolume names asked for so far.
    pub fn requested(&self) -> Vec<String> {
        self.requested.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl WarmDiskSource for ScriptedDiskSource {
    async fn data_volume(
        &self,
        ops: &KubeVirtOps,
        _volume: &VolumeClientConfig,
        _reference: &OciArtifactRef,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<DynamicObject, WarmDiskSourceError> {
        self.requested
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(dv_name.to_owned());
        if self.fail {
            return Err(WarmDiskSourceError::new("the snapshot is unavailable"));
        }
        Ok(ops.build_data_volume_spec(dv_name, json!({"blank": {}}), labels))
    }
}
