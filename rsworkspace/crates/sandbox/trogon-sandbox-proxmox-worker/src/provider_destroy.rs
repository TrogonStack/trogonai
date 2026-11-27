use trogon_sandbox_contract::LeaseId;

use crate::check::ensure;
use crate::{LeaseRecord, ProxmoxProvider, ProxmoxProviderError, VmPhase};

impl ProxmoxProvider {
    /// Destroys the VM `record` describes, once it is proven to be the one this worker created.
    pub(crate) async fn destroy_owned(
        &self,
        lease: &LeaseId,
        record: &LeaseRecord,
    ) -> Result<(), ProxmoxProviderError> {
        match &record.phase {
            VmPhase::Destroyed => return Ok(()),
            VmPhase::DestroyTask { upid } => {
                self.api.wait_task(upid).await?;
                return self.record_destroyed(lease, record).await;
            }
            VmPhase::CloneTask { upid } => {
                self.api.wait_task_terminal(upid).await?;
            }
            VmPhase::Unknown | VmPhase::Ready => {}
        }
        let Some(config) = self.api.config_if_present(record.vm).await? else {
            ensure(
                matches!(record.phase, VmPhase::CloneTask { .. }),
                ProxmoxProviderError::Backend(
                    "VM configuration is absent without a destruction receipt; backend quiescence is unproven",
                ),
            )?;
            return self.record_destroyed(lease, record).await;
        };
        ensure(config["name"] == record.name, ProxmoxProviderError::Foreign("the VM"))?;
        ensure(
            config["lock"] != "clone",
            ProxmoxProviderError::Backend("clone is still running; cleanup must wait for its task"),
        )?;
        let mut settled = record.clone();
        settled.phase = VmPhase::Ready;
        self.settle(lease, &settled).await?;
        // A failed stop is harmless only when the subsequent destroy succeeds.
        let _ = self.api.stop(record.vm).await;
        let upid = self.api.begin_destroy(record.vm).await?;
        settled.phase = VmPhase::DestroyTask { upid: upid.clone() };
        self.settle(lease, &settled).await?;
        self.api.wait_task(&upid).await?;
        self.record_destroyed(lease, &settled).await
    }

    async fn record_destroyed(&self, lease: &LeaseId, record: &LeaseRecord) -> Result<(), ProxmoxProviderError> {
        let mut settled = record.clone();
        settled.phase = VmPhase::Destroyed;
        self.settle(lease, &settled).await
    }

    /// Journals the record and mirrors its phase into the held lease.
    async fn settle(&self, lease: &LeaseId, record: &LeaseRecord) -> Result<(), ProxmoxProviderError> {
        self.save(lease, record).await?;
        if let Some(active) = self.active.lock().await.get_mut(lease) {
            active.phase = record.phase.clone();
        }
        Ok(())
    }
}
