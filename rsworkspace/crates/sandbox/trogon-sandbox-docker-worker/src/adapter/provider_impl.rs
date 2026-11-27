use super::*;

#[async_trait]
impl Provider for DockerWorkerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LocalDocker
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        if self.active.lock().await.contains_key(lease_id.as_str()) {
            return Err(ProviderError::backend("lease assignment is still held or cleaning up"));
        }
        self.lease_locked(req, lease_id).await.map_err(into_provider_error)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        self.cancel_locked(lease_id).await.map_err(into_provider_error)
    }

    async fn snapshot(&self, lease_id: &LeaseId, target: &OciArtifactRef) -> Result<SnapshotReceipt, ProviderError> {
        self.snapshot_locked(lease_id, target)
            .await
            .map_err(into_provider_error)
    }

    async fn checkpoint_running(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ProviderError> {
        let push_target = SnapshotPushTarget {
            registry: target.registry.clone(),
            repository: target.repository.clone(),
            artifact_type: target.artifact_type,
        };
        self.checkpoint_for_lease(lease_id, &push_target)
            .await
            .map_err(into_provider_error)
    }
}

impl DockerWorkerProvider {
    pub(super) async fn snapshot_locked(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> anyhow::Result<SnapshotReceipt> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let active = self.active_registration(lease_id).await?;

        let registry = if target.registry.is_empty() {
            self.default_registry.clone()
        } else {
            target.registry.clone()
        };
        let push_target = SnapshotPushTarget {
            registry: registry.clone(),
            repository: target.repository.clone(),
            artifact_type: target.artifact_type,
        };
        let receipt = self
            .export_container_snapshot(&active.container_id, &push_target)
            .await?;

        // A manual snapshot shuts the sandbox down automatically, so the
        // caller does not need to call `cancel()` separately afterward.
        self.teardown_container(lease_id, &active.container_id, active.sidecar_id.as_deref())
            .await?;
        self.forget_executions(lease_id).await;
        self.rollback_state.forget(lease_id).await;
        self.active.lock().await.remove(lease_id.as_str());
        self.authority_paused
            .lock()
            .await
            .retain(|owned| owned.lease_id() != lease_id || !Self::authority_matches(&active, owned));

        let fresh_reference = OciArtifactRef {
            registry,
            repository: target.repository.clone(),
            digest: receipt.digest.clone(),
            artifact_type: receipt.artifact_type,
        };
        self.refill_warm_pool(receipt.digest.clone(), fresh_reference, active.memory_mib, active.cpu)
            .await;

        Ok(receipt)
    }
}
