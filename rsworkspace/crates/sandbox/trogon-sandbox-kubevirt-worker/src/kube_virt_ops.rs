mod claim_resolution;
mod custody;
mod inspect;
mod manifest;
mod resources;
mod snapshot;
mod warm;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use kube::api::{
    ApiResource, DeleteParams, ListParams, Patch, PatchParams, PostParams, Preconditions, PropagationPolicy,
};
use kube::core::DynamicObject;
use kube::{Api, Client};
use serde_json::Value;
use tokio::time::{Instant, sleep};

use self::inspect::{first_interface_ip, has_condition, nic_binding_findings, owned_by, status_str};
use self::manifest::{object, render_cloud_init_user_data, storage_spec, virtual_machine_manifest};
use self::resources::{data_volume_resource, vm_resource, vmi_resource};
use crate::constants::{
    KUBEVIRT_POLL_INTERVAL, KUBEVIRT_WAIT_TIMEOUT, LEASE_LABEL, TENANT_LABEL, WARM_POOL_IDLE, WARM_POOL_LABEL,
    WORKER_LABEL,
};
use crate::{
    CleanupOutcome, CustodyResourceKind, KubeVirtConfig, KubeVirtOpsError, LeaseJournal, LeaseResources,
    ResourceReceipt, resource_name,
};

/// The KubeVirt and CDI resources one worker manages in its namespace: DataVolumes, VirtualMachines, their
/// instances, VM snapshots, and the warm pool.
///
/// A plain instance only talks to the cluster. One made by [`KubeVirtOps::with_resource_receipts`] also keeps a
/// receipt, the namespace, name and uid the server returned, for every resource it creates or claims, and one made
/// by [`KubeVirtOps::with_durable_receipts`] writes each create intent and receipt to the lease's custody journal,
/// so a restarted worker can still find and settle what it created. Cleanup only deletes resources whose uid
/// matches a receipt.
pub struct KubeVirtOps {
    client: Client,
    config: KubeVirtConfig,
    receipts: Option<Arc<Mutex<LeaseResources>>>,
    journal: Option<Arc<LeaseJournal>>,
}

impl KubeVirtOps {
    pub fn new(client: Client, config: KubeVirtConfig) -> Self {
        Self {
            client,
            config,
            receipts: None,
            journal: None,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn config(&self) -> &KubeVirtConfig {
        &self.config
    }

    /// The VirtualMachine name of a lease.
    pub fn vm_name(&self, lease_id: &str) -> String {
        resource_name("vm", lease_id)
    }

    /// The root DataVolume name of a lease.
    pub fn dv_name(&self, lease_id: &str) -> String {
        resource_name("dv", lease_id)
    }

    /// The VirtualMachineSnapshot name of a lease.
    pub fn snapshot_name(&self, lease_id: &str) -> String {
        resource_name("snap", lease_id)
    }

    /// The labels every resource of a lease carries: its lease, its tenant, and this worker.
    pub fn lease_labels(&self, lease_id: &str, tenant: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            (LEASE_LABEL.to_owned(), lease_id.to_owned()),
            (TENANT_LABEL.to_owned(), tenant.to_owned()),
            (WORKER_LABEL.to_owned(), self.config.worker_id.to_string()),
        ])
    }

    /// A DataVolume named `name` that imports from `source` into a disk of the configured size and class.
    pub fn build_data_volume_spec(&self, name: &str, source: Value, labels: BTreeMap<String, String>) -> DynamicObject {
        let spec = object([("source", source), ("storage", storage_spec(&self.config))]);
        let mut data_volume = DynamicObject::new(name, &data_volume_resource())
            .within(&self.config.namespace)
            .data(object([("spec", spec)]));
        data_volume.metadata.labels = Some(labels);
        data_volume
    }

    pub async fn create_data_volume(&self, data_volume: DynamicObject) -> Result<(), KubeVirtOpsError> {
        self.create_data_volume_owned(data_volume).await?;
        Ok(())
    }

    /// Creates `data_volume` and returns what the server stored. A receipt collector records its receipt, and a
    /// custody journal writes the create intent down before the request and the receipt after it.
    pub async fn create_data_volume_owned(
        &self,
        data_volume: DynamicObject,
    ) -> Result<DynamicObject, KubeVirtOpsError> {
        let name = data_volume
            .metadata
            .name
            .clone()
            .ok_or(KubeVirtOpsError::MissingDataVolumeName)?;
        self.before_create(CustodyResourceKind::DataVolume, &name)?;
        let created = self
            .api(&data_volume_resource())
            .create(&PostParams::default(), &data_volume)
            .await
            .map_err(|error| create_error(error, KubeVirtOpsError::DataVolumeExists))?;
        if let Some(receipts) = &self.receipts {
            let receipt = self.receipt(&name, &created)?;
            lock(receipts).data_volume = Some(receipt);
        }
        self.persist_resources()?;
        Ok(created)
    }

    /// Waits up to [`crate::KUBEVIRT_WAIT_TIMEOUT`] for the DataVolume import to succeed. A held receipt must keep
    /// matching the DataVolume the whole time.
    pub async fn wait_data_volume_ready(&self, name: &str) -> Result<(), KubeVirtOpsError> {
        let api = self.api(&data_volume_resource());
        let deadline = Instant::now() + KUBEVIRT_WAIT_TIMEOUT;
        loop {
            let data_volume = api.get(name).await?;
            if let Some(receipt) = self.resource_receipts().data_volume {
                require(
                    self.receipt(name, &data_volume)? == receipt,
                    KubeVirtOpsError::DataVolumeChanged,
                )?;
            }
            let phase = status_str(&data_volume.data, "phase").unwrap_or_default();
            match phase {
                "Succeeded" => break Ok(()),
                "Failed" | "Error" => {
                    let message = status_str(&data_volume.data, "message").unwrap_or("datavolume import failed");
                    break Err(KubeVirtOpsError::DataVolumeFailed {
                        name: name.to_owned(),
                        message: message.to_owned(),
                    });
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                break Err(KubeVirtOpsError::DataVolumeTimeout {
                    name: name.to_owned(),
                    phase: phase.to_owned(),
                });
            }
            sleep(KUBEVIRT_POLL_INTERVAL).await;
        }
    }

    /// The cloud-init user data for a guest that trusts the bootstrap key, when configured, and `extra_keys`.
    pub fn build_cloud_init_user_data(&self, extra_keys: &[String]) -> String {
        render_cloud_init_user_data(self.config.bootstrap_ssh_pubkey.as_deref(), extra_keys)
    }

    /// A running VirtualMachine that boots `dv_name`, sized to `memory_mib` and, when given, `vcpu_millis`.
    ///
    /// The VM carries `labels`; its instance template also carries [`crate::GUEST_POLICY_LABEL`] so the guest
    /// network policy selects the virt-launcher pod.
    pub fn build_virtual_machine(
        &self,
        vm_name: &str,
        dv_name: &str,
        memory_mib: u32,
        vcpu_millis: Option<u32>,
        cloud_init: &str,
        labels: BTreeMap<String, String>,
    ) -> DynamicObject {
        virtual_machine_manifest(
            &self.config,
            vm_name,
            dv_name,
            memory_mib,
            vcpu_millis,
            cloud_init,
            labels,
        )
    }

    /// Creates `vm` and checks what the server stored. A VM whose NIC bindings would bypass the pod network
    /// policy is deleted again and reported.
    pub async fn create_virtual_machine(&self, vm: DynamicObject) -> Result<(), KubeVirtOpsError> {
        let name = vm
            .metadata
            .name
            .clone()
            .ok_or(KubeVirtOpsError::MissingVirtualMachineName)?;
        let created = self.create_virtual_machine_owned(vm).await?;
        let receipt = self.receipt(&name, &created)?;
        let validation = self.validate_virtual_machine(&created);
        if validation.is_err() {
            self.delete_virtual_machine_confirmed(&receipt).await?;
        }
        validation
    }

    /// Creates `vm` and returns what the server stored, recording its receipt like
    /// [`KubeVirtOps::create_data_volume_owned`].
    pub async fn create_virtual_machine_owned(&self, vm: DynamicObject) -> Result<DynamicObject, KubeVirtOpsError> {
        let name = vm
            .metadata
            .name
            .clone()
            .ok_or(KubeVirtOpsError::MissingVirtualMachineName)?;
        self.before_create(CustodyResourceKind::VirtualMachine, &name)?;
        let created = self
            .api(&vm_resource())
            .create(&PostParams::default(), &vm)
            .await
            .map_err(|error| create_error(error, KubeVirtOpsError::VirtualMachineExists))?;
        if let Some(receipts) = &self.receipts {
            let receipt = self.receipt(&name, &created)?;
            lock(receipts).vm = Some(receipt);
        }
        self.persist_resources()?;
        Ok(created)
    }

    /// Refuses a stored VM unless every guest NIC is a masquerade interface on the pod network.
    pub fn validate_virtual_machine(&self, created: &DynamicObject) -> Result<(), KubeVirtOpsError> {
        let findings = nic_binding_findings(&created.data);
        if findings.is_empty() {
            return Ok(());
        }
        Err(KubeVirtOpsError::UnsafeNicBinding {
            name: created.metadata.name.clone().unwrap_or_default(),
            findings,
        })
    }

    /// The receipt of `object`, which the server returned for `name`. It must be in this worker's namespace and
    /// carry a uid.
    pub fn receipt(&self, name: &str, object: &DynamicObject) -> Result<ResourceReceipt, KubeVirtOpsError> {
        require(
            object.metadata.name.as_deref() == Some(name),
            KubeVirtOpsError::ReceiptName,
        )?;
        require(
            object.metadata.namespace.as_deref() == Some(self.config.namespace.as_str()),
            KubeVirtOpsError::ReceiptNamespace,
        )?;
        let uid = object
            .metadata
            .uid
            .as_ref()
            .filter(|uid| !uid.trim().is_empty())
            .ok_or(KubeVirtOpsError::ReceiptUid)?;
        Ok(ResourceReceipt {
            namespace: self.config.namespace.clone(),
            name: name.to_owned(),
            uid: uid.clone(),
        })
    }

    async fn resource(&self, resource: &ApiResource, name: &str) -> Result<Option<DynamicObject>, KubeVirtOpsError> {
        Ok(found(self.api(resource).get(name).await)?)
    }

    /// The VirtualMachine named `name`, if there is one.
    pub async fn virtual_machine(&self, name: &str) -> Result<Option<DynamicObject>, KubeVirtOpsError> {
        self.resource(&vm_resource(), name).await
    }

    /// The DataVolume named `name`, if there is one.
    pub async fn data_volume(&self, name: &str) -> Result<Option<DynamicObject>, KubeVirtOpsError> {
        self.resource(&data_volume_resource(), name).await
    }

    /// Deletes the resource `receipt` names only while its uid, and `resource_version` when given, still match,
    /// then confirms it is gone. A resource already replaced by another uid counts as absent.
    async fn delete_confirmed(
        &self,
        resource: &ApiResource,
        receipt: &ResourceReceipt,
        resource_version: Option<&str>,
    ) -> Result<CleanupOutcome, KubeVirtOpsError> {
        require(
            receipt.namespace == self.config.namespace && !receipt.uid.trim().is_empty(),
            KubeVirtOpsError::CleanupReceipt,
        )?;
        let api = self.api(resource);
        let Some(observed) = found(api.get(&receipt.name).await)? else {
            return Ok(CleanupOutcome::Absent);
        };
        if self.receipt(&receipt.name, &observed)?.uid != receipt.uid {
            return Ok(CleanupOutcome::Absent);
        }
        if let Some(revision) = resource_version {
            require(
                observed.metadata.resource_version.as_deref() == Some(revision),
                KubeVirtOpsError::WarmRevisionChanged,
            )?;
        }
        let params = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Foreground),
            preconditions: Some(Preconditions {
                uid: Some(receipt.uid.clone()),
                resource_version: resource_version.map(str::to_owned),
            }),
            ..DeleteParams::default()
        };
        let deleted = match api.delete(&receipt.name, &params).await {
            Ok(_) => true,
            Err(kube::Error::Api(status)) if status.code == 404 => return Ok(CleanupOutcome::Absent),
            Err(kube::Error::Api(status)) if status.code == 409 => false,
            Err(error) => return Err(error.into()),
        };
        let outcome = if deleted {
            CleanupOutcome::Deleted
        } else {
            CleanupOutcome::Absent
        };
        match found(api.get(&receipt.name).await)? {
            Some(object) if self.receipt(&receipt.name, &object)?.uid == receipt.uid => {
                Err(KubeVirtOpsError::StillPresent {
                    name: receipt.name.clone(),
                })
            }
            _ => Ok(outcome),
        }
    }

    /// Deletes the VirtualMachine `receipt` names, see [`CleanupOutcome`].
    pub async fn delete_virtual_machine_confirmed(
        &self,
        receipt: &ResourceReceipt,
    ) -> Result<CleanupOutcome, KubeVirtOpsError> {
        self.delete_confirmed(&vm_resource(), receipt, None).await
    }

    /// Deletes the DataVolume `receipt` names, see [`CleanupOutcome`].
    pub async fn delete_data_volume_confirmed(
        &self,
        receipt: &ResourceReceipt,
    ) -> Result<CleanupOutcome, KubeVirtOpsError> {
        self.delete_confirmed(&data_volume_resource(), receipt, None).await
    }

    /// Waits up to [`crate::KUBEVIRT_WAIT_TIMEOUT`] for the instance of `vm_name` to run with a `Ready` condition.
    /// A held VM receipt must stay current and own the instance.
    pub async fn wait_vmi_ready(&self, vm_name: &str) -> Result<(), KubeVirtOpsError> {
        let api = self.api(&vmi_resource());
        let deadline = Instant::now() + KUBEVIRT_WAIT_TIMEOUT;
        loop {
            if let Some(vmi) = found(api.get(vm_name).await)? {
                if let Some(owner) = self.resource_receipts().vm {
                    self.ensure_current_vm(&owner).await?;
                    let owned = vmi.metadata.namespace.as_deref() == Some(owner.namespace.as_str())
                        && owned_by(&vmi, &owner.uid);
                    require(owned, KubeVirtOpsError::VmiOwnerMismatch)?;
                }
                let phase = status_str(&vmi.data, "phase");
                if phase == Some("Running") && has_condition(&vmi.data, "Ready") {
                    break Ok(());
                }
                if phase == Some("Failed") {
                    break Err(KubeVirtOpsError::VmiFailed {
                        name: vm_name.to_owned(),
                    });
                }
            }
            if Instant::now() >= deadline {
                break Err(KubeVirtOpsError::VmiReadyTimeout {
                    name: vm_name.to_owned(),
                });
            }
            sleep(KUBEVIRT_POLL_INTERVAL).await;
        }
    }

    /// The address the guest answers on, from the VMI's reported interfaces.
    ///
    /// The VMI is created with a masquerade interface that forwards port 22, and KubeVirt reports the pod-network
    /// address for it, which is reachable from the worker over the cluster pod network.
    pub async fn vmi_ip(&self, vm_name: &str) -> Result<String, KubeVirtOpsError> {
        let vmi = self.api(&vmi_resource()).get(vm_name).await?;
        first_interface_ip(&vmi.data).ok_or_else(|| KubeVirtOpsError::NoInterfaceAddress {
            name: vm_name.to_owned(),
        })
    }

    /// Whether the instance of `vm_name` exists and is in phase `Running`.
    pub async fn vmi_is_running(&self, vm_name: &str) -> Result<bool, KubeVirtOpsError> {
        let vmi = found(self.api(&vmi_resource()).get(vm_name).await)?;
        Ok(vmi.is_some_and(|vmi| status_str(&vmi.data, "phase") == Some("Running")))
    }

    /// Starts or stops `vm_name` through `spec.running`.
    pub async fn set_virtual_machine_running(&self, vm_name: &str, running: bool) -> Result<(), KubeVirtOpsError> {
        let patch = Patch::Merge(object([("spec", object([("running", Value::Bool(running))]))]));
        self.api(&vm_resource())
            .patch(vm_name, &PatchParams::default(), &patch)
            .await?;
        Ok(())
    }

    /// Waits up to [`crate::KUBEVIRT_WAIT_TIMEOUT`] for the instance of `vm_name` to be gone or out of `Running`.
    pub async fn wait_vmi_stopped(&self, vm_name: &str) -> Result<(), KubeVirtOpsError> {
        let api = self.api(&vmi_resource());
        let deadline = Instant::now() + KUBEVIRT_WAIT_TIMEOUT;
        loop {
            let vmi = found(api.get(vm_name).await)?;
            if vmi.is_none_or(|vmi| status_str(&vmi.data, "phase") != Some("Running")) {
                break Ok(());
            }
            if Instant::now() >= deadline {
                break Err(KubeVirtOpsError::VmiStopTimeout {
                    name: vm_name.to_owned(),
                });
            }
            sleep(KUBEVIRT_POLL_INTERVAL).await;
        }
    }

    /// Deletes `name` with foreground propagation, whoever owns it. A missing VM is not an error.
    pub async fn delete_virtual_machine(&self, name: &str) -> Result<(), KubeVirtOpsError> {
        let params = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Foreground),
            ..DeleteParams::default()
        };
        found(self.api(&vm_resource()).delete(name, &params).await)?;
        Ok(())
    }

    /// Deletes `name`, whoever owns it. A missing DataVolume is not an error.
    pub async fn delete_data_volume(&self, name: &str) -> Result<(), KubeVirtOpsError> {
        found(
            self.api(&data_volume_resource())
                .delete(name, &DeleteParams::default())
                .await,
        )?;
        Ok(())
    }

    fn api(&self, resource: &ApiResource) -> Api<DynamicObject> {
        Api::namespaced_with(self.client.clone(), &self.config.namespace, resource)
    }

    fn before_create(&self, kind: CustodyResourceKind, name: &str) -> Result<(), KubeVirtOpsError> {
        if let Some(journal) = &self.journal {
            journal.before_create(kind, name)?;
        }
        Ok(())
    }
}

fn require(condition: bool, error: KubeVirtOpsError) -> Result<(), KubeVirtOpsError> {
    if condition { Ok(()) } else { Err(error) }
}

/// `None` for a 404, so a missing resource is an answer rather than a failure.
fn found<T>(result: Result<T, kube::Error>) -> Result<Option<T>, kube::Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(kube::Error::Api(status)) if status.code == 404 => Ok(None),
        Err(error) => Err(error),
    }
}

/// A create conflict names the existing resource, anything else is a Kubernetes failure.
fn create_error(error: kube::Error, exists: fn(String) -> KubeVirtOpsError) -> KubeVirtOpsError {
    match error {
        kube::Error::Api(status) if status.code == 409 => exists(status.message.clone()),
        error => KubeVirtOpsError::Kube(error),
    }
}

fn lock(receipts: &Mutex<LeaseResources>) -> MutexGuard<'_, LeaseResources> {
    receipts.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lists the VMs labelled idle in the warm pool.
fn idle_warm_vms() -> ListParams {
    ListParams::default().labels(&format!("{WARM_POOL_LABEL}={WARM_POOL_IDLE}"))
}
