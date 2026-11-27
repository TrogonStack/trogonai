use async_nats::jetstream::Context;
use trogon_sandbox_contract::{
    CancelOwnedLeaseRequest, DispatchTarget, LeaseAttemptIdentity, LeaseError, LeaseId, LeaseRequest, LifecycleEvent,
    ProviderCleanupRequest, ProviderCleanupResponse, ProviderKind, ProviderLease, SandboxId, SandboxPhase, TenantId,
};
use trogon_sandbox_protocol::{AttemptPhase, PhaseTimer, VerifiedVerbScope, lifecycle_subject};

use super::{E2bDispatchError, E2bWorkerProvider, LeaseAccess};
use crate::LeaseSandbox;
use crate::assignments::{Assignment, Recovery};

impl E2bWorkerProvider {
    /// Whether the durable assignment is held by `tenant` for exactly the native sandbox `sandbox_id`.
    pub(super) fn names_held(assignment: &Assignment, tenant: &TenantId, sandbox_id: &str) -> bool {
        assignment.tenant == *tenant && assignment.sandbox_id.as_deref() == Some(sandbox_id)
    }

    /// The sandbox a command verb may reach, once its signed scope matches the held tenant and assignment.
    pub(crate) async fn sandbox_for_command(
        &self,
        lease_id: &LeaseId,
        scope: Option<&VerifiedVerbScope>,
        require_launch_cancellation: bool,
    ) -> Result<Option<LeaseSandbox>, LeaseError> {
        let denied = || LeaseError::InvalidSpec {
            reason: "command authority does not match the held tenant and worker assignment".into(),
        };
        let unavailable = || LeaseError::Backend {
            message: "durable command assignment authority is unavailable".into(),
        };
        if scope.is_some_and(|scope| {
            scope
                .scope()
                .worker_id
                .as_ref()
                .is_some_and(|worker| self.worker_id.get().is_none_or(|held| held != worker))
        }) {
            return Ok(None);
        }
        let (tenant, sandbox_id) = match self.active.lock().await.get(lease_id.as_str()) {
            Some(held) if held.access == LeaseAccess::Ready => (held.tenant.clone(), held.sandbox_id.clone()),
            _ => return Ok(None),
        };
        let Some(scope) = scope else {
            if require_launch_cancellation {
                return Err(denied());
            }
            return Ok(self.sandbox_for_lease(lease_id).await);
        };
        let scope = scope.scope();
        if scope.tenant != tenant || scope.lease_id != *lease_id {
            return Err(denied());
        }
        if let Some(ledger) = self.dispatch_ledger() {
            let worker = scope.worker_id.clone().ok_or_else(denied)?;
            let attempt = scope.attempt_id.clone().ok_or_else(denied)?;
            let epoch = scope.epoch.ok_or_else(denied)?;
            let identity = LeaseAttemptIdentity {
                lease_id: lease_id.clone(),
                provider: ProviderKind::E2b,
                target: DispatchTarget {
                    worker_id: worker,
                    attempt_id: attempt,
                    epoch: Some(epoch),
                },
            };
            let state = ledger
                .try_states()
                .await
                .map_err(|_| unavailable())?
                .into_iter()
                .find(|state| state.identity == identity)
                .ok_or_else(denied)?;
            let receipt = state.receipt.as_ref().ok_or_else(denied)?;
            if state.phase != AttemptPhase::Ready
                || receipt.sandbox_id.as_str() != sandbox_id
                || receipt.worker_id.as_ref() != Some(&identity.target.worker_id)
                || receipt.attempt_id.as_ref() != Some(&identity.target.attempt_id)
                || receipt.epoch != identity.target.epoch
            {
                return Err(denied());
            }
            let assignment = self
                .assignments
                .load(&identity)
                .await
                .map_err(|_| unavailable())?
                .ok_or_else(denied)?;
            Self::names_held(&assignment, &tenant, sandbox_id.as_str())
                .then_some(())
                .ok_or_else(denied)?;
        } else if require_launch_cancellation {
            return Err(denied());
        }
        Ok(self.sandbox_for_lease(lease_id).await)
    }

    /// Allocates the sandbox one signed dispatch targets, journaling the attempt and publishing its lifecycle.
    pub(crate) async fn dispatch_targeted(
        &self,
        request: &LeaseRequest,
        identity: &LeaseAttemptIdentity,
        context: &Context,
    ) -> Result<ProviderLease, E2bDispatchError> {
        let ledger = self.dispatch_ledger().ok_or(E2bDispatchError::NotConfigured)?;
        if identity.provider != ProviderKind::E2b
            || &identity.target.worker_id != ledger.worker_id()
            || identity.target.epoch.is_none()
        {
            return Err(E2bDispatchError::TargetMismatch);
        }
        self.assignments
            .reserve(&request.tenant, identity, Some(Recovery::from_request(request)))
            .await?;
        let receipt = ledger
            .dispatch(identity, || async {
                let subject = lifecycle_subject(&identity.lease_id);
                // A lease id the protocol would not accept as a sandbox id has no lifecycle events to name it in.
                let named = SandboxId::new(identity.lease_id.as_str()).ok();
                if let Some(sandbox_id) = &named {
                    crate::publish_lifecycle::publish_lifecycle(
                        context,
                        &subject,
                        &LifecycleEvent::Started {
                            sandbox_id: sandbox_id.clone(),
                        },
                    )
                    .await;
                }
                let timer = PhaseTimer::start(SandboxPhase::Allocate);
                crate::publish_lifecycle::publish_lifecycle(context, &subject, &timer.phase_started()).await;
                let result = self.allocate_lease(request, &identity.lease_id, Some(identity)).await;
                match &result {
                    Ok(receipt) => {
                        crate::publish_lifecycle::publish_lifecycle(context, &subject, &timer.complete()).await;
                        crate::publish_lifecycle::publish_lifecycle(
                            context,
                            &subject,
                            &LifecycleEvent::Completed {
                                sandbox_id: receipt.sandbox_id.clone(),
                            },
                        )
                        .await;
                    }
                    Err(error) => {
                        crate::publish_lifecycle::publish_lifecycle(context, &subject, &timer.fail(error.to_string()))
                            .await;
                        if let Some(sandbox_id) = named {
                            crate::publish_lifecycle::publish_lifecycle(
                                context,
                                &subject,
                                &LifecycleEvent::Failed {
                                    sandbox_id,
                                    error: error.to_string(),
                                },
                            )
                            .await;
                        }
                    }
                }
                result
            })
            .await?;
        let assignment = self
            .assignments
            .load(identity)
            .await?
            .ok_or(E2bDispatchError::ReadyDispatchLacksAssignment)?;
        Self::names_held(&assignment, &request.tenant, receipt.sandbox_id.as_str())
            .then_some(())
            .ok_or(E2bDispatchError::ReadyDispatchChanged)?;
        Ok(receipt)
    }

    /// Tears down the sandbox one signed cleanup targets and records the result in the dispatch ledger.
    pub(crate) async fn cleanup_targeted(
        &self,
        request: &ProviderCleanupRequest,
        tenant: &TenantId,
    ) -> ProviderCleanupResponse {
        let failed = |error: E2bDispatchError| ProviderCleanupResponse::Failed {
            identity: request.identity.clone(),
            message: error.to_string(),
        };
        let Some(ledger) = self.dispatch_ledger() else {
            return failed(E2bDispatchError::CleanupNotConfigured);
        };
        if &request.identity.target.worker_id != ledger.worker_id() {
            return failed(E2bDispatchError::CleanupForeignWorker);
        }
        if let Err(error) = self.assignments.reserve(tenant, &request.identity, None).await {
            return failed(error.into());
        }
        ledger
            .cleanup(request, |state| async move {
                let receipt = state.receipt.ok_or(E2bDispatchError::CleanupWithoutReceipt)?;
                self.assignments
                    .bind_native(&state.identity, receipt.sandbox_id.as_str())
                    .await?;
                let assignment = self
                    .assignments
                    .load(&state.identity)
                    .await?
                    .ok_or(E2bDispatchError::CleanupLacksOwnership)?;
                Self::names_held(&assignment, tenant, receipt.sandbox_id.as_str())
                    .then_some(())
                    .ok_or(E2bDispatchError::CleanupAuthorityChanged)?;
                let gate = self.lease_gate(&state.identity.lease_id);
                let _held = gate.lock().await;
                if let Some(active) = self.active.lock().await.get_mut(state.identity.lease_id.as_str()) {
                    if active.tenant != *tenant || active.sandbox_id != receipt.sandbox_id.as_str() {
                        return Err(E2bDispatchError::CleanupDoesNotMatchHeld);
                    }
                    active.access = LeaseAccess::CleanupOnly;
                }
                let owned = CancelOwnedLeaseRequest {
                    tenant: tenant.clone(),
                    lease_id: state.identity.lease_id.clone(),
                    sandbox_id: receipt.sandbox_id,
                };
                let stored = self.cleanup_receipts.begin(&owned).await?;
                if !stored.absent() {
                    self.cancel_journaled_resource(&owned, stored).await?;
                }
                self.forget_exact(&owned.lease_id, owned.sandbox_id.as_str()).await;
                Ok::<(), E2bDispatchError>(())
            })
            .await
    }
}
