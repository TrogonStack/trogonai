use std::collections::BTreeMap;

use kube::api::{Patch, PatchParams};
use serde_json::Value;
use trogon_sandbox_contract::Digest;

use super::inspect::warm_vm_has_memory;
use super::manifest::{object, string_map};
use super::resources::{vm_resource, vmi_resource};
use super::{KubeVirtOps, idle_warm_vms};
use crate::constants::{
    DIGEST_ANNOTATION, LEASE_LABEL, ROOT_DV_ANNOTATION, ROOT_DV_UID_ANNOTATION, TENANT_LABEL, WARM_MEMORY_ANNOTATION,
    WARM_POOL_IDLE, WARM_POOL_LABEL, WARM_POOL_SLOT_LABEL, WARM_POOL_TENANT, WARM_POOL_UNASSIGNED_LEASE, WORKER_LABEL,
};
use crate::{KubeVirtOpsError, ResourceReceipt};

impl KubeVirtOps {
    /// The labels of an idle warm VM: idle in the pool, owned by the warm pool tenant, no lease yet, this worker.
    pub fn warm_pool_idle_labels(&self, _digest: &Digest) -> BTreeMap<String, String> {
        BTreeMap::from([
            (WARM_POOL_LABEL.to_owned(), WARM_POOL_IDLE.to_owned()),
            (TENANT_LABEL.to_owned(), WARM_POOL_TENANT.to_owned()),
            (LEASE_LABEL.to_owned(), WARM_POOL_UNASSIGNED_LEASE.to_owned()),
            (WORKER_LABEL.to_owned(), self.config.worker_id.to_string()),
        ])
    }

    /// The annotations of a warm VM: the digest it was restored from, its root disk receipt, and its memory.
    pub fn warm_pool_annotations(
        &self,
        digest: &Digest,
        disk: &ResourceReceipt,
        memory_mib: u32,
    ) -> BTreeMap<String, String> {
        BTreeMap::from([
            (DIGEST_ANNOTATION.to_owned(), digest.as_str().to_owned()),
            (ROOT_DV_ANNOTATION.to_owned(), disk.name.clone()),
            (ROOT_DV_UID_ANNOTATION.to_owned(), disk.uid.clone()),
            (WARM_MEMORY_ANNOTATION.to_owned(), memory_mib.to_string()),
        ])
    }

    /// Merges `annotations` into the VM's annotations.
    pub async fn set_virtual_machine_annotations(
        &self,
        vm_name: &str,
        annotations: BTreeMap<String, String>,
    ) -> Result<(), KubeVirtOpsError> {
        let metadata = object([("annotations", Value::Object(string_map(&annotations)))]);
        self.api(&vm_resource())
            .patch(
                vm_name,
                &PatchParams::default(),
                &Patch::Merge(object([("metadata", metadata)])),
            )
            .await?;
        Ok(())
    }

    /// The name and root DataVolume of the first idle warm VM restored from `digest` that fits `memory_mib`,
    /// trusting labels and annotations alone. [`KubeVirtOps::find_confirmed_warm_vm`] also checks receipts.
    pub async fn find_idle_warm_vmi(
        &self,
        digest: &Digest,
        memory_mib: u32,
    ) -> Result<Option<(String, String)>, KubeVirtOpsError> {
        let listing = self.api(&vm_resource()).list(&idle_warm_vms()).await?;
        Ok(listing.items.into_iter().find_map(|vm| {
            let name = vm.metadata.name.unwrap_or_default();
            let annotations = vm.metadata.annotations.unwrap_or_default();
            let fits = annotations
                .get(DIGEST_ANNOTATION)
                .is_some_and(|annotated| annotated == digest.as_str())
                && warm_vm_has_memory(&annotations, memory_mib);
            fits.then(|| {
                let dv_name = annotations
                    .get(ROOT_DV_ANNOTATION)
                    .cloned()
                    .unwrap_or_else(|| self.dv_name(&name));
                (name, dv_name)
            })
        }))
    }

    /// The name, digest, and root DataVolume of every idle warm VM with a digest annotation. A malformed digest
    /// fails the listing.
    pub async fn list_idle_warm_vmis(&self) -> Result<Vec<(String, Digest, String)>, KubeVirtOpsError> {
        let listing = self.api(&vm_resource()).list(&idle_warm_vms()).await?;
        let mut idle = Vec::new();
        for vm in listing.items {
            let name = vm.metadata.name.unwrap_or_default();
            let annotations = vm.metadata.annotations.unwrap_or_default();
            let Some(digest) = annotations.get(DIGEST_ANNOTATION) else {
                continue;
            };
            let digest = Digest::new(digest.as_str())?;
            let dv_name = annotations
                .get(ROOT_DV_ANNOTATION)
                .cloned()
                .unwrap_or_else(|| self.dv_name(&name));
            idle.push((name, digest, dv_name));
        }
        Ok(idle)
    }

    /// Relabels a warm VM, its template, and its running instance to `lease_labels`, without a fence.
    /// [`KubeVirtOps::claim_confirmed_warm_vm`] is the fenced claim.
    pub async fn claim_warm_vmi(
        &self,
        vm_name: &str,
        lease_labels: BTreeMap<String, String>,
    ) -> Result<(), KubeVirtOpsError> {
        let mut labels = lease_labels;
        labels.remove(WARM_POOL_LABEL);
        labels.remove(WARM_POOL_SLOT_LABEL);
        let labels = Value::Object(string_map(&labels));
        let template = object([("metadata", object([("labels", labels.clone())]))]);
        let patch = Patch::Merge(object([
            ("metadata", object([("labels", labels.clone())])),
            ("spec", object([("template", template)])),
        ]));
        self.api(&vm_resource())
            .patch(vm_name, &PatchParams::default(), &patch)
            .await?;
        let instances = self.api(&vmi_resource());
        if instances.get(vm_name).await.is_ok() {
            let patch = Patch::Merge(object([("metadata", object([("labels", labels)]))]));
            let _ignored = instances.patch(vm_name, &PatchParams::default(), &patch).await;
        }
        Ok(())
    }
}
