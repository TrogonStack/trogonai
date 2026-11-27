//! In-place rollback of a lease to a snapshot.
use trogon_sandbox_contract::{LeaseId, OciArtifactRef, SandboxImage, SandboxRollbackResponse};

use super::DockerWorkerProvider;

impl DockerWorkerProvider {
    /// Whether a persisted request exists for `lease_id`.
    pub async fn has_rollback_state(&self, lease_id: &LeaseId) -> bool {
        self.rollback_state.load(lease_id).await.is_some()
    }

    /// Rebuilds a Legacy lease's container from `snapshot`, keeping the lease id
    /// and the shape it was created with. The snapshot is pulled and checked
    /// before the old container is touched, so a bad snapshot leaves the lease
    /// as it was. A failure after that point has already removed the old
    /// container, and the denial says so.
    pub async fn rollback_for_lease(&self, lease_id: &LeaseId, snapshot: &OciArtifactRef) -> SandboxRollbackResponse {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let backend =
            |message: String| SandboxRollbackResponse::Denied(trogon_sandbox_contract::LeaseError::Backend { message });
        if let Some(ledger) = self.dispatch_ledger() {
            match ledger.has_lease(lease_id).await {
                Ok(true) => return SandboxRollbackResponse::Unsupported,
                Ok(false) => {}
                Err(error) => {
                    return backend(format!("rollback ownership ledger is unavailable: {error:#}"));
                }
            }
        }
        let live = {
            let active = self.active.lock().await;
            match active.get(lease_id.as_str()) {
                Some(lease) if lease.dispatch_target.is_some() => {
                    return SandboxRollbackResponse::Unsupported;
                }
                Some(lease) => Some((*lease.request).clone()),
                None => None,
            }
        };
        // No live container: after a restart the startup sweep removed it, and
        // the persisted request, when kept, rebuilds the lease from the snapshot.
        let (mut request, container_gone) = match live {
            Some(request) => (request, false),
            None => match self.rollback_state.load(lease_id).await {
                Some(request) => (request, true),
                None => {
                    return SandboxRollbackResponse::Denied(trogon_sandbox_contract::LeaseError::LeaseNotFound {
                        lease_id: lease_id.clone(),
                    });
                }
            },
        };
        if let Err(e) = self.restore_snapshot_image(snapshot).await {
            return backend(format!(
                "rollback snapshot is not restorable, sandbox left as it was: {e:#}"
            ));
        }
        let image = SandboxImage::Snapshot {
            reference: snapshot.clone(),
            parent: None,
        };
        request.spec = match super::spec_edit::with_image(request.spec.clone(), image) {
            Ok(spec) => spec,
            Err(e) => {
                return backend(format!(
                    "rollback snapshot does not fit the lease, sandbox left as it was: {e}"
                ));
            }
        };
        if !container_gone && let Err(e) = self.cancel_locked(lease_id).await {
            return backend(format!("rollback could not remove the old sandbox: {e:#}"));
        }
        // Cancel dropped the saved request; keep the rolled-back shape so a
        // retry after a failed rebuild still finds it.
        self.rollback_state.remember(lease_id, &request).await;
        match self.lease_locked(&request, lease_id).await {
            Ok(_) => SandboxRollbackResponse::Done,
            Err(e) => backend(format!("rollback failed after the old sandbox was removed: {e:#}")),
        }
    }
}
