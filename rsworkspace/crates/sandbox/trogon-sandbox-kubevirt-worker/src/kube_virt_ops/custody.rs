use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use kube::api::{ApiResource, Patch, PatchParams};
use kube::core::DynamicObject;
use serde_json::Value;
use trogon_sandbox_contract::Digest;

use super::claim_resolution::ClaimResolution;
use super::inspect::{annotation, label, owned_by, warm_vm_has_memory};
use super::manifest::{object, string_map};
use super::resources::{data_volume_resource, vm_resource, vmi_resource};
use super::{KubeVirtOps, idle_warm_vms, lock, require};
use crate::constants::{
    CLAIM_ANNOTATION, DIGEST_ANNOTATION, ROOT_DV_ANNOTATION, ROOT_DV_UID_ANNOTATION, WARM_POOL_IDLE, WARM_POOL_LABEL,
    WARM_POOL_SLOT_LABEL,
};
use crate::{
    CustodyPhase, CustodyToken, KubeVirtOpsError, LeaseJournal, LeaseResources, ResourceReceipt, WarmCandidate,
    WarmClaimIntent,
};

impl KubeVirtOps {
    /// A copy that keeps an in-memory receipt for every resource it creates or claims.
    pub fn with_resource_receipts(&self) -> Self {
        Self {
            client: self.client.clone(),
            config: self.config.clone(),
            receipts: Some(Arc::default()),
            journal: None,
        }
    }

    /// A copy that starts from `resources` and writes every create intent and receipt to `journal`.
    pub fn with_durable_receipts(&self, journal: Arc<LeaseJournal>, resources: LeaseResources) -> Self {
        Self {
            client: self.client.clone(),
            config: self.config.clone(),
            receipts: Some(Arc::new(Mutex::new(resources))),
            journal: Some(journal),
        }
    }

    /// Writes the held receipts to the custody journal, when there is one.
    pub fn persist_resources(&self) -> Result<(), KubeVirtOpsError> {
        if let Some(journal) = &self.journal {
            journal.resources(&self.resource_receipts())?;
        }
        Ok(())
    }

    fn replace_receipts(&self, resources: &LeaseResources) -> Result<(), KubeVirtOpsError> {
        if let Some(receipts) = &self.receipts {
            *lock(receipts) = resources.clone();
        }
        self.persist_resources()
    }

    /// The receipts held so far, empty without a receipt collector.
    pub fn resource_receipts(&self) -> LeaseResources {
        self.receipts
            .as_ref()
            .map(|receipts| lock(receipts).clone())
            .unwrap_or_default()
    }

    /// The shared receipt collector, so a lease can keep reading what this copy records.
    pub fn receipt_log(&self) -> Result<Arc<Mutex<LeaseResources>>, KubeVirtOpsError> {
        self.receipts.clone().ok_or(KubeVirtOpsError::UnscopedReceipts)
    }

    /// The VirtualMachine `receipt` names, as long as it is still the one the receipt was taken from.
    pub async fn ensure_current_vm(&self, receipt: &ResourceReceipt) -> Result<DynamicObject, KubeVirtOpsError> {
        let current = self
            .virtual_machine(&receipt.name)
            .await?
            .ok_or(KubeVirtOpsError::HeldVmAbsent)?;
        require(
            self.receipt(&receipt.name, &current)? == *receipt,
            KubeVirtOpsError::HeldVmReplaced,
        )?;
        Ok(current)
    }

    /// Deletes everything `resources` names, settling a pending warm claim first, and keeps the receipts current
    /// after every step so a failure leaves exactly what is still owned.
    ///
    /// With a custody journal the lease moves to the cleanup phase first, which is refused while a create is
    /// unresolved.
    pub async fn release_resources(&self, resources: &LeaseResources) -> Result<(), KubeVirtOpsError> {
        self.replace_receipts(resources)?;
        if let Some(journal) = &self.journal {
            let record = journal.record()?;
            require(
                record.create.is_none() && record.phase != CustodyPhase::Unresolved,
                KubeVirtOpsError::OwnershipUnresolved,
            )?;
            journal.set_phase(CustodyPhase::Cleanup)?;
        }
        let mut owned = resources.clone();
        if let Some(intent) = &resources.pending_claim {
            match self.resolve_warm_claim(intent).await? {
                ClaimResolution::Owned => owned = LeaseResources::claiming(&intent.candidate),
                ClaimResolution::NotOwned => owned.pending_claim = None,
                ClaimResolution::Settled => return self.replace_receipts(&LeaseResources::default()),
            }
            self.replace_receipts(&owned)?;
        }
        if let Some(vm) = owned.vm.take() {
            self.delete_virtual_machine_confirmed(&vm).await?;
            self.replace_receipts(&owned)?;
        }
        if let Some(data_volume) = owned.data_volume.take() {
            self.delete_data_volume_confirmed(&data_volume).await?;
            self.replace_receipts(&owned)?;
        }
        Ok(())
    }

    /// What `resources` still owns in the cluster after a restart: a pending warm claim is settled, and receipts
    /// whose resource is gone are dropped. A resource replaced under the same name is refused unless nothing else
    /// is owned, because the replacement may share the original's dependencies.
    pub async fn reconcile_receipts(&self, resources: &LeaseResources) -> Result<LeaseResources, KubeVirtOpsError> {
        let mut owned = resources.clone();
        if let Some(intent) = &resources.pending_claim {
            let resolution = self.resolve_warm_claim(intent).await?;
            if !matches!(resolution, ClaimResolution::Owned) {
                return Ok(LeaseResources::default());
            }
            owned = LeaseResources::claiming(&intent.candidate);
        }
        let (vm, vm_replaced) = self.current_receipt(&vm_resource(), owned.vm.take()).await?;
        let (data_volume, data_volume_replaced) = self
            .current_receipt(&data_volume_resource(), owned.data_volume.take())
            .await?;
        owned.vm = vm;
        owned.data_volume = data_volume;
        require(
            !(vm_replaced || data_volume_replaced) || owned.is_empty(),
            KubeVirtOpsError::ReplacementShared,
        )?;
        Ok(owned)
    }

    /// `receipt` while the resource it names is still the one it was taken from, and whether another resource
    /// replaced it under the same name.
    async fn current_receipt(
        &self,
        resource: &ApiResource,
        receipt: Option<ResourceReceipt>,
    ) -> Result<(Option<ResourceReceipt>, bool), KubeVirtOpsError> {
        let Some(receipt) = receipt else {
            return Ok((None, false));
        };
        let Some(current) = self.resource(resource, &receipt.name).await? else {
            return Ok((None, false));
        };
        let kept = self.receipt(&receipt.name, &current)? == receipt;
        Ok((kept.then_some(receipt), !kept))
    }

    /// Every idle warm VM whose root DataVolume is still the one its annotations name.
    pub async fn confirmed_idle_warm_vms(&self) -> Result<Vec<WarmCandidate>, KubeVirtOpsError> {
        let listing = self.api(&vm_resource()).list(&idle_warm_vms()).await?;
        let mut candidates = Vec::new();
        for vm in &listing.items {
            if let Some(candidate) = self.idle_candidate(vm).await? {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    async fn idle_candidate(&self, vm: &DynamicObject) -> Result<Option<WarmCandidate>, KubeVirtOpsError> {
        let Some(candidate) = self.annotated_candidate(vm) else {
            return Ok(None);
        };
        let disk = &candidate.data_volume;
        let current = self.data_volume(&disk.name).await?;
        let confirmed = current.is_some_and(|current| self.receipt(&disk.name, &current).is_ok_and(|r| r == *disk));
        Ok(confirmed.then_some(candidate))
    }

    /// The candidate an idle, undeleted VM describes through its receipt, revision, and annotations.
    fn annotated_candidate(&self, vm: &DynamicObject) -> Option<WarmCandidate> {
        let metadata = &vm.metadata;
        let idle = metadata.deletion_timestamp.is_none() && label(metadata, WARM_POOL_LABEL) == Some(WARM_POOL_IDLE);
        let name = metadata.name.as_deref().filter(|_| idle)?;
        let receipt = self.receipt(name, vm).ok()?;
        let revision = metadata
            .resource_version
            .as_ref()
            .filter(|revision| !revision.is_empty())?;
        let digest = Digest::new(annotation(metadata, DIGEST_ANNOTATION)?).ok()?;
        let data_volume = ResourceReceipt {
            namespace: self.config.namespace.clone(),
            name: annotation(metadata, ROOT_DV_ANNOTATION)?.to_owned(),
            uid: annotation(metadata, ROOT_DV_UID_ANNOTATION)?.to_owned(),
        };
        Some(WarmCandidate {
            vm: receipt,
            data_volume,
            resource_version: revision.clone(),
            digest,
        })
    }

    /// The first confirmed idle warm VM restored from `digest` that booted with at least `memory_mib`.
    pub async fn find_confirmed_warm_vm(
        &self,
        digest: &Digest,
        memory_mib: u32,
    ) -> Result<Option<WarmCandidate>, KubeVirtOpsError> {
        let candidates = self.confirmed_idle_warm_vms().await?;
        for candidate in candidates.into_iter().filter(|candidate| candidate.digest == *digest) {
            let current = self.ensure_current_vm(&candidate.vm).await?;
            let annotations = current.metadata.annotations.as_ref();
            if annotations.is_some_and(|annotations| warm_vm_has_memory(annotations, memory_mib)) {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// What the cluster shows about `intent` when its patch reply was not observed.
    async fn resolve_warm_claim(&self, intent: &WarmClaimIntent) -> Result<ClaimResolution, KubeVirtOpsError> {
        let vm = &intent.candidate.vm;
        let current = match self.virtual_machine(&vm.name).await? {
            Some(current) if self.receipt(&vm.name, &current)? == *vm => current,
            _ => {
                let disk = &intent.candidate.data_volume;
                let remains = match self.data_volume(&disk.name).await? {
                    Some(current) => self.receipt(&disk.name, &current)? == *disk,
                    None => false,
                };
                require(!remains, KubeVirtOpsError::ClaimUnknownWithDisk)?;
                return Ok(ClaimResolution::Settled);
            }
        };
        let token = intent.token.to_string();
        if annotation(&current.metadata, CLAIM_ANNOTATION) == Some(token.as_str()) {
            let labelled = intent
                .labels
                .iter()
                .all(|(key, value)| label(&current.metadata, key) == Some(value.as_str()));
            require(labelled, KubeVirtOpsError::ClaimLabelConflict)?;
            return Ok(ClaimResolution::Owned);
        }
        let unchanged =
            current.metadata.resource_version.as_deref() == Some(intent.candidate.resource_version.as_str());
        require(!unchanged, KubeVirtOpsError::ClaimUnknownAtRevision)?;
        Ok(ClaimResolution::NotOwned)
    }

    /// Claims `candidate` for the lease `labels` describe, at most once across every worker.
    ///
    /// The claim is written to the receipts before the cluster is asked, then applied as one patch fenced on the
    /// VM's uid and resource version that relabels the VM and its template and marks it with a fresh claim token.
    /// When the reply is lost, the cluster is read back to tell whether the claim took effect. Returns whether this
    /// lease now owns the VM and its DataVolume.
    pub async fn claim_confirmed_warm_vm(
        &self,
        candidate: WarmCandidate,
        labels: BTreeMap<String, String>,
    ) -> Result<bool, KubeVirtOpsError> {
        let receipts = self.receipts.as_ref().ok_or(KubeVirtOpsError::NoReceiptCollector)?;
        let intent = WarmClaimIntent {
            candidate,
            token: CustodyToken::generate(),
            labels,
        };
        lock(receipts).pending_claim = Some(intent.clone());
        self.persist_resources()?;
        let token = intent.token.to_string();
        let mut claim_labels = string_map(&intent.labels);
        claim_labels.insert(WARM_POOL_LABEL.to_owned(), Value::Null);
        claim_labels.insert(WARM_POOL_SLOT_LABEL.to_owned(), Value::Null);
        let claim_labels = Value::Object(claim_labels);
        let metadata = object([
            ("uid", intent.candidate.vm.uid.as_str().into()),
            ("resourceVersion", intent.candidate.resource_version.as_str().into()),
            ("labels", claim_labels.clone()),
            ("annotations", object([(CLAIM_ANNOTATION, token.as_str().into())])),
        ]);
        let template = object([("metadata", object([("labels", claim_labels.clone())]))]);
        let patch = Patch::Merge(object([
            ("metadata", metadata),
            ("spec", object([("template", template)])),
        ]));
        let vm_name = &intent.candidate.vm.name;
        let result = self
            .api(&vm_resource())
            .patch(vm_name, &PatchParams::default(), &patch)
            .await;
        let resolution = match result {
            Ok(current) => {
                let confirmed = self.receipt(vm_name, &current)? == intent.candidate.vm
                    && annotation(&current.metadata, CLAIM_ANNOTATION) == Some(token.as_str());
                require(confirmed, KubeVirtOpsError::ClaimUnconfirmed)?;
                ClaimResolution::Owned
            }
            Err(source) => {
                self.resolve_warm_claim(&intent)
                    .await
                    .map_err(|inspect| KubeVirtOpsError::ClaimPending {
                        source,
                        inspect: Box::new(inspect),
                    })?
            }
        };
        let claimed = matches!(resolution, ClaimResolution::Owned);
        {
            let mut receipts = lock(receipts);
            if claimed {
                receipts.vm = Some(intent.candidate.vm.clone());
                receipts.data_volume = Some(intent.candidate.data_volume.clone());
            }
            receipts.pending_claim = None;
        }
        self.persist_resources()?;
        if claimed {
            self.relabel_claimed_vmi(&intent.candidate.vm, claim_labels).await?;
        }
        Ok(claimed)
    }

    /// Moves the claim labels onto the running instance of a claimed VM, fenced on the instance's uid and
    /// revision. An instance the VM does not own is refused.
    async fn relabel_claimed_vmi(&self, vm: &ResourceReceipt, labels: Value) -> Result<(), KubeVirtOpsError> {
        let api = self.api(&vmi_resource());
        let Some(vmi) = api.get_opt(&vm.name).await? else {
            return Ok(());
        };
        let receipt = self.receipt(&vm.name, &vmi)?;
        require(owned_by(&vmi, &vm.uid), KubeVirtOpsError::VmiRelabelForeign)?;
        let revision = vmi
            .metadata
            .resource_version
            .clone()
            .ok_or(KubeVirtOpsError::VmiRelabelNoRevision)?;
        let metadata = object([
            ("uid", receipt.uid.into()),
            ("resourceVersion", revision.into()),
            ("labels", labels),
        ]);
        api.patch(
            &receipt.name,
            &PatchParams::default(),
            &Patch::Merge(object([("metadata", metadata)])),
        )
        .await?;
        Ok(())
    }

    /// Deletes an idle warm VM and its DataVolume while the VM is still the idle revision `candidate` saw.
    ///
    /// Returns `false` without deleting anything once the VM was claimed or changed. A VM that is already gone
    /// still has its DataVolume deleted.
    pub async fn evict_confirmed_idle_vm(&self, candidate: &WarmCandidate) -> Result<bool, KubeVirtOpsError> {
        let current = self.virtual_machine(&candidate.vm.name).await?;
        let current = current
            .map(|object| {
                self.receipt(&candidate.vm.name, &object)
                    .map(|receipt| (receipt, object))
            })
            .transpose()?;
        if let Some((receipt, current)) = current.filter(|(receipt, _)| *receipt == candidate.vm) {
            let idle = label(&current.metadata, WARM_POOL_LABEL) == Some(WARM_POOL_IDLE);
            let unchanged = current.metadata.resource_version.as_deref() == Some(candidate.resource_version.as_str());
            if !(idle && unchanged) {
                return Ok(false);
            }
            self.delete_confirmed(&vm_resource(), &receipt, Some(&candidate.resource_version))
                .await?;
        }
        if let Some(receipts) = &self.receipts {
            lock(receipts).data_volume = Some(candidate.data_volume.clone());
        }
        self.delete_data_volume_confirmed(&candidate.data_volume).await?;
        Ok(true)
    }
}
