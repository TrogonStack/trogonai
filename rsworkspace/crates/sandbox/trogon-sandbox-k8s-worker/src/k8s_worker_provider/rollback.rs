//! In-place rollback: rebuild a running lease's sandbox from a snapshot while
//! the lease id stays the same.
//!
//! The Job or warm pod behind a lease is immutable in the ways that matter
//! here (its image and restore volume are fixed at creation), so a rollback
//! deletes the old sandbox and launches a new one from the request the lease
//! was created with, with the snapshot as its image. That request is held in
//! [`ActiveK8sLease`], and optionally mirrored in a Secret so it survives a
//! worker restart.

use trogon_sandbox_contract::{LeaseError, LeaseId, OciArtifactRef, SandboxRollbackResponse};
use trogon_sandbox_cosign::{into_pull_error, verify_if_configured};

use crate::active_k8s_lease::ActiveK8sLease;
use crate::constants::OLD_SANDBOX_GONE_TIMEOUT;
use crate::rollback_request::request_with_snapshot;
use crate::snapshot::pull_rootfs;
use crate::{K8sWorkerError, K8sWorkerProvider};

fn backend(message: String) -> SandboxRollbackResponse {
    SandboxRollbackResponse::Denied(LeaseError::Backend { message })
}

impl K8sWorkerProvider {
    /// Rebuilds `lease_id`'s sandbox from `snapshot`, keeping the lease id and
    /// the request it was created with. The snapshot is verified and pulled
    /// before the old sandbox is touched, so a bad snapshot leaves the lease
    /// as it was. A failure after that point has already removed the old
    /// sandbox; the lease stays on this worker so the rollback can be retried
    /// or the lease cancelled, and the denial says so.
    pub async fn rollback_for_lease(&self, lease_id: &LeaseId, snapshot: &OciArtifactRef) -> SandboxRollbackResponse {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let Some(old) = self.active.lock().await.get(lease_id).cloned() else {
            return SandboxRollbackResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            });
        };
        let Some(request) = old.request.clone() else {
            return backend(
                "this worker has no copy of the request the lease was created from; \
                 start it with --persist-rollback-state to keep it across restarts"
                    .to_string(),
            );
        };
        if let Err(e) = self.check_snapshot(snapshot).await {
            return backend(format!(
                "rollback snapshot is not restorable, sandbox left as it was: {e}"
            ));
        }
        let relaunch = match request_with_snapshot(&request, snapshot) {
            Ok(relaunch) => relaunch,
            Err(e) => {
                return backend(format!("rollback could not rebuild the lease request: {e}"));
            }
        };

        if let Err(e) = self.remove_old_sandbox(&old).await {
            return backend(format!("rollback could not remove the old sandbox: {e}"));
        }
        self.active.lock().await.remove(lease_id);
        match self.lease_locked(&relaunch, lease_id).await {
            Ok(_) => SandboxRollbackResponse::Done,
            Err(e) => {
                // Keep the lease so the caller can retry the rollback or
                // cancel it; the entry's pod is gone until a launch succeeds.
                let kept = ActiveK8sLease {
                    // Releasing the sandbox deleted the stored copy.
                    request_persisted: false,
                    ..old
                };
                self.active.lock().await.insert(lease_id.clone(), kept);
                backend(format!("rollback failed after the old sandbox was removed: {e}"))
            }
        }
    }

    /// Verifies the snapshot's signature and pulls it, which is everything
    /// that can fail about a snapshot before a launch.
    async fn check_snapshot(&self, snapshot: &OciArtifactRef) -> Result<(), K8sWorkerError> {
        verify_if_configured(self.cosign_verifier.as_ref(), snapshot)
            .await
            .map_err(into_pull_error)?;
        pull_rootfs(self.oci.as_ref(), snapshot).await?;
        Ok(())
    }

    /// Deletes the sandbox behind `old` and waits until the cluster has
    /// finished, because the relaunch of a cold lease reuses the Job's name.
    /// The lease stays in the table, and no lifecycle event says it ended.
    async fn remove_old_sandbox(&self, old: &ActiveK8sLease) -> Result<(), K8sWorkerError> {
        self.release(old, None).await?;
        if !old.is_warm_pod {
            self.inner
                .wait_for_job_gone(&old.sandbox_id, OLD_SANDBOX_GONE_TIMEOUT)
                .await?;
        }
        Ok(())
    }
}
