use tracing::warn;

use crate::active_lease::ActiveLease;
use crate::{CustodyPhase, KubeVirtProvider, KubeVirtProviderError, LeaseJournal};

impl KubeVirtProvider {
    /// Settles what the custody directory records from before the last restart.
    ///
    /// A ready lease whose exact VM and root volume are still there is held again. Whatever else a record names is
    /// kept for cleanup, and the call fails while any allocation cannot be proven settled, so the worker does not
    /// serve on top of unresolved custody.
    pub async fn recover_leases(&self) -> Result<(), KubeVirtProviderError> {
        let mut unresolved = 0;
        for record in self.journal.all()? {
            let lease = &record.lease_id;
            let _gate = self.authority_gate(lease).lock_owned().await;
            let handle = LeaseJournal::handle(&self.journal, &record);
            if record.create.is_some() || record.phase == CustodyPhase::Unresolved {
                self.retained
                    .lock()
                    .await
                    .insert(lease.clone(), record.resources.clone());
                unresolved += 1;
                continue;
            }
            let resources = match self.ops.reconcile_receipts(&record.resources).await {
                Ok(resources) => resources,
                Err(error) => {
                    self.retained
                        .lock()
                        .await
                        .insert(lease.clone(), record.resources.clone());
                    warn!(lease_id = %lease, %error, "KubeVirt restart custody remains unresolved");
                    unresolved += 1;
                    continue;
                }
            };
            let ready = record.phase == CustodyPhase::Ready;
            if ready && (resources.vm.is_none() || resources.data_volume.is_none()) {
                handle.set_phase(CustodyPhase::Cleanup)?;
            }
            handle.resources(&resources)?;
            if resources.is_empty() {
                handle.finish()?;
                continue;
            }
            if ready && let (Some(vm), Some(disk)) = (resources.vm.as_ref(), resources.data_volume.as_ref()) {
                let actual_vm = self.ops.ensure_current_vm(vm).await?;
                let actual_disk = self
                    .ops
                    .data_volume(&disk.name)
                    .await?
                    .ok_or(KubeVirtProviderError::HeldDiskMissing)?;
                let labels = self.journal.labels(&record);
                let vm_labels = actual_vm.metadata.labels.unwrap_or_default();
                let scoped = labels.iter().all(|(key, value)| vm_labels.get(key) == Some(value));
                if self.ops.receipt(&disk.name, &actual_disk)? != *disk || !scoped {
                    return Err(KubeVirtProviderError::ForeignResource);
                }
                let ops = self.ops.with_durable_receipts(handle, resources.clone());
                ops.wait_vmi_ready(&vm.name).await?;
                let active = ActiveLease {
                    vm_name: vm.name.clone(),
                    resources,
                };
                self.active.lock().await.insert(lease.clone(), active);
            } else {
                handle.set_phase(CustodyPhase::Cleanup)?;
                self.retained.lock().await.insert(lease.clone(), resources);
            }
        }
        if unresolved == 0 {
            Ok(())
        } else {
            Err(KubeVirtProviderError::Unresolved { count: unresolved })
        }
    }
}
