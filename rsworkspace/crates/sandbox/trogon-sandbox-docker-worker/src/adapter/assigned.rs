use super::DockerWorkerProvider;
use anyhow::{Context, ensure};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, LifecycleEvent, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId, TenantId,
};
use trogon_sandbox_protocol::AttemptLedger;
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_protocol::global_verifier;

impl DockerWorkerProvider {
    pub fn with_dispatch_ledger(mut self, ledger: Arc<AttemptLedger>) -> anyhow::Result<Self> {
        ensure!(
            ledger.worker_id().as_str() == self.worker_id,
            "Docker ledger and actual worker identity differ"
        );
        self.dispatch_ledger = Some(ledger);
        Ok(self)
    }

    pub fn dispatch_ledger(&self) -> Option<&Arc<AttemptLedger>> {
        self.dispatch_ledger.as_ref()
    }

    pub fn mark_intake_ready(&self) {
        self.intake_ready.store(true, Ordering::Release);
        self.intake_notify.notify_waiters();
    }

    pub async fn wait_for_intake(&self) {
        loop {
            let notified = self.intake_notify.notified();
            if self.intake_ready.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub async fn targeted_intake_ready(&self) -> bool {
        if !self.intake_ready.load(Ordering::Acquire) || global_verifier().fresh_command_launch_fence().is_err() {
            return false;
        }
        match self.dispatch_ledger() {
            Some(ledger) => ledger.try_states().await.is_ok(),
            None => false,
        }
    }

    pub async fn require_legacy_intake(&self, lease: &LeaseId, assigned: bool) -> anyhow::Result<()> {
        if let Some(ledger) = self.dispatch_ledger() {
            ensure!(!assigned, "assigned Docker dispatch requires exact targeted intake");
            ensure!(
                ledger
                    .try_states()
                    .await?
                    .iter()
                    .all(|state| state.identity.lease_id != *lease),
                "Docker lease belongs to targeted intake, including its cleanup tombstone"
            );
        }
        Ok(())
    }

    fn validate_target_identity(&self, identity: &LeaseAttemptIdentity) -> anyhow::Result<()> {
        ensure!(
            identity.provider == ProviderKind::LocalDocker
                && identity.target.worker_id.as_str() == self.worker_id
                && identity.target.epoch.is_some_and(|epoch| epoch.get() > 0),
            "targeted Docker request must match its actual worker and nonzero epoch"
        );
        Ok(())
    }

    fn validate_target(&self, identity: &LeaseAttemptIdentity) -> anyhow::Result<()> {
        self.validate_target_identity(identity)?;
        global_verifier().fresh_command_launch_fence()?;
        Ok(())
    }

    async fn verify_targeted_native(&self, lease: &LeaseId, receipt: &ProviderLease) -> anyhow::Result<()> {
        ensure!(
            receipt.sandbox_id.as_str().len() == 64
                && receipt.sandbox_id.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()),
            "targeted Docker receipt is not canonical"
        );
        let held = self
            .active
            .lock()
            .await
            .get(lease.as_str())
            .cloned()
            .context("targeted native registration missing")?;
        ensure!(
            held.container_id == receipt.sandbox_id.as_str()
                && held.dispatch_target_bound
                && held
                    .dispatch_target
                    .as_ref()
                    .is_some_and(|target| receipt.worker_id.as_ref() == Some(&target.worker_id)
                        && receipt.attempt_id.as_ref() == Some(&target.attempt_id)
                        && receipt.epoch == target.epoch),
            "targeted Docker receipt differs from its current assignment"
        );
        self.verify_held_native_birth(lease, &held).await
    }

    pub(super) async fn retain_allocated_assignment(
        &self,
        lease: &LeaseId,
        container_id: &str,
        assignment: Option<&LeaseAttemptIdentity>,
    ) -> anyhow::Result<ProviderLease> {
        let mut receipt = ProviderLease {
            sandbox_id: SandboxId::new(container_id)?,
            evidence_subject: trogon_sandbox_protocol::subjects::evidence_subject(lease),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        };
        if let Some(identity) = assignment {
            self.validate_target_identity(identity)?;
            ensure!(identity.lease_id == *lease, "allocation receipt changed lease identity");
            {
                let mut active = self.active.lock().await;
                let held = active
                    .get_mut(lease.as_str())
                    .context("allocated Docker lease has no registration")?;
                ensure!(held.container_id == container_id, "allocated native receipt changed");
                held.dispatch_target = Some(identity.target.clone());
                held.dispatch_target_bound = true;
                held.warm_pool_eligible = false;
            }
            receipt.worker_id = Some(identity.target.worker_id.clone());
            receipt.attempt_id = Some(identity.target.attempt_id.clone());
            receipt.epoch = identity.target.epoch;
            self.dispatch_ledger()
                .context("targeted receipt ledger unavailable")?
                .record_backend_receipt(identity, receipt.clone())
                .await?;
        }
        Ok(receipt)
    }

    pub async fn dispatch_targeted(&self, payload: &ProviderDispatchPayload) -> anyhow::Result<ProviderLease> {
        let target = payload
            .target
            .clone()
            .context("targeted Docker dispatch requires an assignment")?;
        let identity = LeaseAttemptIdentity {
            lease_id: payload.lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            target,
        };
        self.validate_target(&identity)?;
        let ledger = self
            .dispatch_ledger()
            .context("Docker targeted intake is unavailable")?;
        let receipt = ledger
            .dispatch(&identity, || async {
                let _gate = self.authority_gate(&payload.lease_id).lock_owned().await;
                ensure!(
                    !self.active.lock().await.contains_key(payload.lease_id.as_str()),
                    "Docker lease is already held"
                );
                let lifecycle = crate::nats_lifecycle::NatsLifecycleEmitter::new(
                    self.js.clone(),
                    trogon_sandbox_protocol::lifecycle_subject(&payload.lease_id),
                );
                lifecycle
                    .emit(LifecycleEvent::Started {
                        sandbox_id: SandboxId::new(payload.lease_id.as_str())?,
                    })
                    .await;
                let provision =
                    trogon_sandbox_protocol::PhaseTimer::start(trogon_sandbox_contract::SandboxPhase::Allocate);
                lifecycle.emit(provision.phase_started()).await;
                let receipt = match self
                    .lease_locked_with_assignment(&payload.request, &payload.lease_id, Some(&identity))
                    .await
                {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        lifecycle.emit(provision.fail(error.to_string())).await;
                        lifecycle
                            .emit(LifecycleEvent::Failed {
                                sandbox_id: SandboxId::new(payload.lease_id.as_str())?,
                                error: error.to_string(),
                            })
                            .await;
                        return Err(error);
                    }
                };
                self.verify_targeted_native(&payload.lease_id, &receipt).await?;
                lifecycle.emit(provision.complete()).await;
                Ok(receipt)
            })
            .await?;
        let _gate = self.authority_gate(&payload.lease_id).lock_owned().await;
        ensure!(
            self.active
                .lock()
                .await
                .get(payload.lease_id.as_str())
                .is_some_and(|held| held.container_id == receipt.sandbox_id.as_str()
                    && held.dispatch_target_bound
                    && held.dispatch_target.as_ref() == Some(&identity.target)
                    && *held.request == payload.request),
            "targeted Docker replay lost its exact held registration"
        );
        ensure!(
            receipt.worker_id.as_ref() == Some(&identity.target.worker_id)
                && receipt.attempt_id.as_ref() == Some(&identity.target.attempt_id)
                && receipt.epoch == identity.target.epoch,
            "targeted Docker receipt lost its signed assignment"
        );
        self.verify_targeted_native(&payload.lease_id, &receipt).await?;
        crate::nats_lifecycle::NatsLifecycleEmitter::new(
            self.js.clone(),
            trogon_sandbox_protocol::lifecycle_subject(&payload.lease_id),
        )
        .emit(LifecycleEvent::Completed {
            sandbox_id: receipt.sandbox_id.clone(),
        })
        .await;
        Ok(receipt)
    }

    pub async fn cleanup_targeted(
        &self,
        request: &ProviderCleanupRequest,
        tenant: &TenantId,
    ) -> ProviderCleanupResponse {
        let failed = |error: anyhow::Error| ProviderCleanupResponse::Failed {
            identity: request.identity.clone(),
            message: error.to_string(),
        };
        if let Err(error) = self.validate_target_identity(&request.identity) {
            return failed(error);
        }
        let Some(ledger) = self.dispatch_ledger() else {
            return failed(anyhow::anyhow!("Docker targeted cleanup is unavailable"));
        };
        ledger
            .cleanup(request, |state| async move {
                let receipt = state
                    .receipt
                    .context("started Docker allocation has no retained native receipt")?;
                ensure!(
                    receipt.worker_id.as_ref() == Some(&state.identity.target.worker_id)
                        && receipt.attempt_id.as_ref() == Some(&state.identity.target.attempt_id)
                        && receipt.epoch == state.identity.target.epoch,
                    "Docker cleanup receipt changed its exact assignment"
                );
                let _gate = self.authority_gate(&state.identity.lease_id).lock_owned().await;
                let held = self.active.lock().await.get(state.identity.lease_id.as_str()).cloned();
                match held {
                    Some(held) => {
                        ensure!(
                            held.request.tenant == *tenant
                                && held.container_id == receipt.sandbox_id.as_str()
                                && held.dispatch_target_bound
                                && held.dispatch_target.as_ref() == Some(&state.identity.target),
                            "Docker cleanup differs from its held tenant, assignment or native receipt"
                        );
                        self.cancel_locked(&state.identity.lease_id).await?;
                        if let Some(sidecar) = held.sidecar_id {
                            self.inner.engine().remove(&sidecar).await?;
                            self.confirm_container_absence(&sidecar).await?;
                        }
                    }
                    None => {
                        ensure!(
                            matches!(
                                self.docker.inspect_container(receipt.sandbox_id.as_str(), None).await,
                                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
                            ),
                            "unheld Docker cleanup has no exact native absence receipt"
                        );
                    }
                }
                self.confirm_container_absence(receipt.sandbox_id.as_str()).await?;
                Ok(())
            })
            .await
    }
}
