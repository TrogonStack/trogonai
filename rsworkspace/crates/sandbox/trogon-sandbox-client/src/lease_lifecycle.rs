use trogon_sandbox_contract::{
    DeleteLeaseRequest, DeleteLeaseResponse, LeaseId, OciArtifactRef, OperationId, PauseLeaseRequest,
    PauseLeaseResponse, ResumeLeaseRequest, ResumeLeaseResponse, RollbackLeaseRequest, RollbackLeaseResponse,
    StopLeaseRequest, StopLeaseResponse, SuspendLeaseRequest, SuspendLeaseResponse, SuspendRequestLevel, TenantId,
    WakeLeaseRequest, WakeLeaseResponse,
};
use trogon_sandbox_protocol::subjects;

use crate::lease_timeouts::{QUICK_VERB_TIMEOUT, SLOW_VERB_TIMEOUT};
use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Permanently deletes `lease_id`, purging anything that would let it be resumed later.
    /// Distinct from [`Self::cancel`], which preserves resumable state.
    pub async fn delete_lease(&self, tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError> {
        self.delete_lease_with_operation(tenant, lease_id, None).await
    }

    /// [`Self::delete_lease`] under a caller-chosen operation id. After a timeout, repeat the
    /// call with the same id to replay the stored outcome, or read it with
    /// [`Self::get_operation`]; the delete never runs twice.
    pub async fn delete_lease_with_operation(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        operation_id: Option<OperationId>,
    ) -> Result<(), ClientError> {
        let request = DeleteLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            operation_id,
        };
        match self
            .send(&subjects::lease_delete(lease_id), &request, QUICK_VERB_TIMEOUT)
            .await?
        {
            DeleteLeaseResponse::Deleted { .. } => Ok(()),
            DeleteLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Stops `lease_id`'s current session while keeping it (and anything
    /// tagged under its `name`) resumable later. Distinct from
    /// [`Self::delete_lease`], which irreversibly purges resumable state.
    pub async fn stop_lease(&self, tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError> {
        let request = StopLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
        };
        match self
            .send(&subjects::lease_stop(lease_id), &request, QUICK_VERB_TIMEOUT)
            .await?
        {
            StopLeaseResponse::Stopped => Ok(()),
            StopLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Pauses `lease_id` in place, keeping its id and state (see
    /// [`PauseLeaseRequest`]). Idempotent. `PauseLeaseResponse::Unsupported`
    /// comes back as `Ok` because it is a capability answer, not a failure.
    pub async fn pause_lease(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        max_paused_seconds: Option<u32>,
    ) -> Result<PauseLeaseResponse, ClientError> {
        self.pause_lease_with_operation(tenant, lease_id, max_paused_seconds, None)
            .await
    }

    /// [`Self::pause_lease`] under a caller-chosen operation id. After a timeout, repeat the
    /// call with the same id to replay the stored outcome, or read it with
    /// [`Self::get_operation`]; the pause never runs twice.
    pub async fn pause_lease_with_operation(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        max_paused_seconds: Option<u32>,
        operation_id: Option<OperationId>,
    ) -> Result<PauseLeaseResponse, ClientError> {
        let request = PauseLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            max_paused_seconds,
            operation_id,
        };
        match self
            .send(&subjects::lease_pause(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            PauseLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Resumes a paused `lease_id`. Idempotent: a running lease is returned
    /// unchanged.
    pub async fn resume_lease(&self, tenant: TenantId, lease_id: &LeaseId) -> Result<ResumeLeaseResponse, ClientError> {
        self.resume_lease_with_operation(tenant, lease_id, None).await
    }

    /// [`Self::resume_lease`] under a caller-chosen operation id. After a timeout, repeat the
    /// call with the same id to replay the stored outcome, or read it with
    /// [`Self::get_operation`]; the resume never runs twice.
    pub async fn resume_lease_with_operation(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        operation_id: Option<OperationId>,
    ) -> Result<ResumeLeaseResponse, ClientError> {
        let request = ResumeLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            operation_id,
        };
        match self
            .send(&subjects::lease_resume(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            ResumeLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Suspends `lease_id` to the level its provider offers, releasing host
    /// resources while keeping its id (see [`SuspendLeaseRequest`]). Idempotent.
    /// A provider that cannot suspend to the level asked answers
    /// `LeaseError::SuspendUnsupported`.
    pub async fn suspend_lease(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        level: SuspendRequestLevel,
        max_suspended_seconds: Option<u32>,
    ) -> Result<SuspendLeaseResponse, ClientError> {
        self.suspend_lease_with_operation(tenant, lease_id, level, max_suspended_seconds, None)
            .await
    }

    /// [`Self::suspend_lease`] under a caller-chosen operation id. After a timeout, repeat the
    /// call with the same id to replay the stored outcome, or read it with
    /// [`Self::get_operation`]; the suspend never runs twice.
    pub async fn suspend_lease_with_operation(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        level: SuspendRequestLevel,
        max_suspended_seconds: Option<u32>,
        operation_id: Option<OperationId>,
    ) -> Result<SuspendLeaseResponse, ClientError> {
        let request = SuspendLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            level,
            max_suspended_seconds,
            operation_id,
        };
        match self
            .send(&subjects::lease_suspend(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            SuspendLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Wakes a suspended `lease_id`, restoring the run time it had left.
    /// Idempotent: a running lease is returned unchanged. A wake already in
    /// flight answers `LeaseError::LeaseWaking`, which carries a retry delay.
    pub async fn wake_lease(&self, tenant: TenantId, lease_id: &LeaseId) -> Result<WakeLeaseResponse, ClientError> {
        self.wake_lease_with_operation(tenant, lease_id, None).await
    }

    /// [`Self::wake_lease`] under a caller-chosen operation id. After a timeout, repeat the
    /// call with the same id to replay the stored outcome, or read it with
    /// [`Self::get_operation`]; the wake never runs twice.
    pub async fn wake_lease_with_operation(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        operation_id: Option<OperationId>,
    ) -> Result<WakeLeaseResponse, ClientError> {
        let request = WakeLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            operation_id,
        };
        match self
            .send(&subjects::lease_wake(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            WakeLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Resets `lease_id` to `snapshot` while keeping its id and deadline. The
    /// sandbox is rebuilt, so its current state is lost. `Unsupported` means the
    /// lease's provider cannot do this in place.
    pub async fn rollback_lease(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        snapshot: OciArtifactRef,
        operation_id: Option<OperationId>,
    ) -> Result<RollbackLeaseResponse, ClientError> {
        let request = RollbackLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            snapshot,
            operation_id,
        };
        match self
            .send(&subjects::lease_rollback(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            RollbackLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }
}
