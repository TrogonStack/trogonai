use std::sync::Arc;

use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, LifecycleEvent, ProviderLease, SandboxId, SandboxImage, SandboxPhase, WorkspaceSource,
};
use trogon_sandbox_cosign::{into_pull_error, verify_if_configured};
use trogon_sandbox_protocol::{
    LifecycleEmitter, lifecycle_subject, reject_unsupported_privileged, run_phase, subjects::evidence_subject,
};
use trogon_sandbox_providers::{
    ImageRef, K8sLaunchOptions, K8sMicroVmProvider, K8sResourceKind, job_name, lease_timeout, restore_secret_name,
    s3_volume_secret_name, translate_lease_request_to_sandbox_spec,
};
use trogon_std::env::SystemEnv;

use crate::active_k8s_lease::ActiveK8sLease;
use crate::constants::POD_START_TIMEOUT;
use crate::lease_request_for_translate::lease_request_for_translate;
use crate::policy_rules::{effective_policy, ensure_policy_enforceable};
use crate::snapshot::pull_rootfs;
use crate::volume::sync_s3_prefix_to_tar;
use crate::{K8sWorkerError, K8sWorkerProvider, NatsLifecycleEmitter, profile};

impl K8sWorkerProvider {
    pub(super) async fn lease_locked(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> Result<ProviderLease, K8sWorkerError> {
        self.worker_id()?;
        if self.active.lock().await.contains_key(lease_id) {
            return Err(K8sWorkerError::LeaseAlreadyHeld);
        }
        if self.cleanup.lock().await.contains_key(lease_id) {
            return Err(K8sWorkerError::CleanupPending);
        }
        let lease_sandbox = SandboxId::new(lease_id.as_str())?;
        let lifecycle = NatsLifecycleEmitter::new(self.js.clone(), lifecycle_subject(lease_id));
        lifecycle
            .emit(LifecycleEvent::Started {
                sandbox_id: lease_sandbox.clone(),
            })
            .await;
        let inner = self.inner.with_resource_receipts();
        let result = run_phase(Some(&lifecycle), SandboxPhase::Allocate, None, || {
            self.allocate(req, lease_id, &inner)
        })
        .await;
        if result.is_err() {
            let resources = inner.resource_receipts();
            if !resources.is_empty()
                && let Err(error) = self.inner.release_resources(&resources).await
            {
                self.cleanup.lock().await.insert(lease_id.clone(), resources);
                tracing::warn!(%lease_id, %error, "retaining cleanup after failed Kubernetes allocation");
            }
        }
        lifecycle
            .emit(match &result {
                Ok(lease) => LifecycleEvent::Completed {
                    sandbox_id: lease.sandbox_id.clone(),
                },
                Err(error) => LifecycleEvent::Failed {
                    sandbox_id: lease_sandbox,
                    error: error.to_string(),
                },
            })
            .await;
        result
    }

    pub(super) async fn allocate(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        inner: &K8sMicroVmProvider,
    ) -> Result<ProviderLease, K8sWorkerError> {
        reject_unsupported_privileged(&req.spec, &profile())?;
        ensure_policy_enforceable(req.spec.network_policy())?;
        let policy = effective_policy(req.spec.network_policy());
        if let SandboxImage::Snapshot { reference, .. } = &req.spec.image()
            && let Some(warm_pool) = &self.warm_pool
            && warm_pool.fits(req.spec.vcpu_millis(), req.spec.memory_mib().get())
            && let Some(claim) = warm_pool
                .try_claim_with_receipts(
                    inner,
                    &reference.digest,
                    lease_id.as_str(),
                    req.tenant.as_str(),
                    &policy,
                    lease_timeout(req),
                )
                .await?
        {
            let owner = inner
                .resource_receipts()
                .into_iter()
                .find(|receipt| receipt.kind() == K8sResourceKind::Pod && receipt.name() == claim.pod_name.as_str())
                .ok_or(K8sWorkerError::WarmClaimWithoutReceipt)?;
            self.digest_cache.insert(reference.digest.clone());
            warm_pool.remember_source(reference);
            let stored = if self.persist_rollback_state {
                self.persist_request(inner, &owner, lease_id, req).await
            } else {
                None
            };
            let request_persisted = stored.is_some();
            self.active.lock().await.insert(
                lease_id.clone(),
                ActiveK8sLease {
                    owner,
                    resources: inner.resource_receipts(),
                    sandbox_id: claim.pod_name.clone(),
                    pod_name: claim.pod_name.clone(),
                    is_warm_pod: true,
                    request: Some(Arc::new(req.clone())),
                    request_persisted,
                },
            );
            let evidence_subject = evidence_subject(lease_id);
            return Ok(ProviderLease {
                epoch: None,
                sandbox_id: SandboxId::new(claim.pod_name)?,
                evidence_subject,
                worker_id: self.worker_id()?,
                attempt_id: None,
            });
        }

        let translate_req = lease_request_for_translate(req)?;
        let mut spec = translate_lease_request_to_sandbox_spec(&translate_req, lease_id)?;
        if let SandboxImage::Snapshot { parent, .. } = &req.spec.image() {
            let image = parent
                .as_ref()
                .map_or_else(|| self.default_image.clone(), |p| p.allowlist_string());
            spec.image = ImageRef::new(image);
        }

        let predicted_job = job_name(&spec.repo.owner, &spec.repo.name, &spec.resolved_sha);

        let mut launch_opts = K8sLaunchOptions {
            lease_id: Some(lease_id.as_str().to_string()),
            tenant_id: Some(req.tenant.as_str().to_string()),
            keep_alive: true,
            ..Default::default()
        };

        if let SandboxImage::Snapshot { reference, .. } = &req.spec.image() {
            verify_if_configured(self.cosign_verifier.as_ref(), reference)
                .await
                .map_err(into_pull_error)?;
            let tar_bytes = pull_rootfs(self.oci.as_ref(), reference).await?;
            self.digest_cache.insert(reference.digest.clone());
            if let Some(warm_pool) = &self.warm_pool {
                warm_pool.remember_source(reference);
            }
            launch_opts.digest_label = Some(reference.digest.to_string());
            let secret = restore_secret_name(&predicted_job);
            inner
                .create_workspace_tar_secret(&secret, &predicted_job, &tar_bytes)
                .await?;
            launch_opts.restore_secret_name = Some(secret.clone());
        }

        if let WorkspaceSource::S3Volume {
            bucket, prefix, mode, ..
        } = &req.spec.workspace()
        {
            let tar_bytes = sync_s3_prefix_to_tar(&SystemEnv, bucket, prefix, *mode).await?;
            let secret = s3_volume_secret_name(&predicted_job);
            inner
                .create_workspace_tar_secret(&secret, &predicted_job, &tar_bytes)
                .await?;
            launch_opts.s3_volume_secret_name = Some(secret.clone());
        }

        let lifecycle = Arc::new(NatsLifecycleEmitter::new(self.js.clone(), lifecycle_subject(lease_id)));
        let runtime_class = self.inner.runtime_class_for(req.isolation_tier).to_string();
        let inner = inner
            .with_lifecycle_emitter(Some(lifecycle))
            .with_runtime_class_override(runtime_class);
        let handle = inner.launch_with_options(spec, launch_opts).await?;
        let sandbox_id = handle.id().to_string();
        let owner = inner
            .resource_receipts()
            .into_iter()
            .find(|receipt| receipt.kind() == K8sResourceKind::Job && receipt.name() == sandbox_id)
            .ok_or(K8sWorkerError::JobWithoutReceipt)?;
        let pod = inner.wait_for_owned_job_pod(&owner, POD_START_TIMEOUT).await?;
        let pod_name = pod.name().to_string();

        let stored = if self.persist_rollback_state {
            self.persist_request(&inner, &owner, lease_id, req).await
        } else {
            None
        };
        let request_persisted = stored.is_some();
        self.active.lock().await.insert(
            lease_id.clone(),
            ActiveK8sLease {
                owner,
                resources: inner.resource_receipts(),
                sandbox_id: sandbox_id.clone(),
                pod_name,
                is_warm_pod: false,
                request: Some(Arc::new(req.clone())),
                request_persisted,
            },
        );
        let evidence_subject = evidence_subject(lease_id);
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(sandbox_id)?,
            evidence_subject,
            worker_id: self.worker_id()?,
            attempt_id: None,
        })
    }

    pub(super) async fn release(
        &self,
        active: &ActiveK8sLease,
        lifecycle: Option<&NatsLifecycleEmitter>,
    ) -> Result<(), K8sWorkerError> {
        run_phase(
            lifecycle,
            SandboxPhase::Reclaim,
            Some(active.sandbox_id.clone()),
            || async { Ok(self.inner.release_resources(&active.resources).await?) },
        )
        .await
    }

    pub(super) async fn cancel_locked(&self, lease_id: &LeaseId) -> Result<(), K8sWorkerError> {
        let active = self.active.lock().await.get(lease_id).cloned();
        if let Some(active) = &active {
            let lifecycle = NatsLifecycleEmitter::new(self.js.clone(), lifecycle_subject(lease_id));
            self.release(active, Some(&lifecycle)).await?;
        }
        let resources = self.cleanup.lock().await.get(lease_id).cloned();
        if let Some(resources) = resources {
            self.inner.release_resources(&resources).await?;
        }
        self.active.lock().await.remove(lease_id);
        self.cleanup.lock().await.remove(lease_id);
        self.forget_lease_state(lease_id).await;
        Ok(())
    }

    pub(super) async fn forget_lease_state(&self, lease_id: &LeaseId) {
        self.launches
            .lock()
            .await
            .retain(|_, launch| launch.lease_id != *lease_id);
        self.executions.dispose_lease(lease_id).await;
        self.ssh_sessions.lock().await.retain(|_, (lease, _)| lease != lease_id);
    }
}
