use async_trait::async_trait;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId, SandboxPhase,
};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, HeldLease, Provider, ProviderError, PurgeOutcome,
    reject_unsupported_privileged, run_phase, subjects,
};

use crate::active_lease::ActiveLease;
use crate::base::Base;
use crate::clone_target::CloneTarget;
use crate::constants::{LEASE_LABEL, TAG_LABEL_PREFIX, WORKSPACE_MODE, WORKSPACE_PATH};
use crate::lifecycle::auto_intervals;
use crate::native_options::native_params;
use crate::network::{require_unrestricted_egress, translate_network_policy};
use crate::profile::PROFILE;
use crate::settle::settle;
use crate::{
    DaytonaApi, DaytonaApiError, DaytonaProviderError, DaytonaWorkerProvider, GitCloneRequest, NatsLifecycleEmitter,
    NewSandbox, SandboxRecord, SandboxState,
};

impl DaytonaWorkerProvider {
    fn request(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> Result<(NewSandbox, Option<CloneTarget>), DaytonaProviderError> {
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
        require_unrestricted_egress(req.spec.network_policy())?;
        if !req.spec.mounts().is_empty() {
            return Err(DaytonaProviderError::MountsUnsupported);
        }
        if req.spec.vcpu_millis().is_some() || req.spec.disk_mib().is_some() {
            return Err(LeaseError::InvalidSpec {
                reason: "a Daytona snapshot fixes the sandbox size; vcpu and disk cannot be set per lease".to_owned(),
            }
            .into());
        }
        let clone = CloneTarget::parse(req.spec.workspace())?;
        let network = translate_network_policy(req.spec.network_policy())?;
        let native = native_params(req.native.as_ref())?;
        let idle = auto_intervals(req.spec.idle_timeout_seconds().map(std::num::NonZeroU32::get));
        let base = Base::for_image(req.spec.image(), &self.default_snapshot)?;
        let mut new = NewSandbox::new(base.into_name());
        new.env = req.spec.env().clone();
        new.labels = req
            .tags
            .iter()
            .map(|(k, v)| (format!("{TAG_LABEL_PREFIX}{k}"), v.to_owned()))
            .collect();
        new.labels.insert(LEASE_LABEL.to_owned(), lease_id.as_str().to_owned());
        new.class = native.class;
        new.gpu = native.gpu;
        new.public = native.public;
        new.target = req.region.clone();
        new.auto_stop_interval = native.auto_stop_interval.or(idle.auto_stop);
        new.auto_archive_interval = native.auto_archive_interval.or(idle.auto_archive);
        new.auto_delete_interval = native.auto_delete_interval.or(idle.auto_delete);
        new.network_block_all = network.block_all;
        new.network_allow_list = network.allow_list;
        Ok((new, clone))
    }

    /// Waits for the new sandbox to start and puts the workspace in it.
    async fn prepare(
        &self,
        record: &SandboxRecord,
        clone: Option<&CloneTarget>,
        emitter: Option<&NatsLifecycleEmitter>,
    ) -> Result<SandboxId, DaytonaProviderError> {
        let sandbox_id = SandboxId::new(record.id.clone())?;
        run_phase(emitter, SandboxPhase::BootRuntime, None, || {
            settle(&self.api, &record.id, SandboxState::Started, self.wait)
        })
        .await?;
        run_phase(emitter, SandboxPhase::PrepareWorkspace, None, || {
            self.workspace(record, clone)
        })
        .await?;
        Ok(sandbox_id)
    }

    /// Puts the workspace directory in a started sandbox: a clone at the commit, or an empty folder.
    async fn workspace(&self, record: &SandboxRecord, clone: Option<&CloneTarget>) -> Result<(), DaytonaProviderError> {
        match clone {
            Some(target) => {
                let (username, password) = target.credentials.clone().unzip();
                self.api
                    .git_clone(
                        &record.id,
                        &GitCloneRequest {
                            url: target.url.clone(),
                            path: WORKSPACE_PATH.to_owned(),
                            commit_id: Some(target.commit.clone()),
                            username,
                            password,
                        },
                    )
                    .await?;
            }
            None => {
                self.api
                    .create_folder(&record.id, WORKSPACE_PATH, WORKSPACE_MODE)
                    .await?
            }
        }
        Ok(())
    }

    async fn launch(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, DaytonaProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.active.lock().await.contains_key(lease_id.as_str()) {
            return Err(DaytonaProviderError::AlreadyHeld);
        }
        let (new, clone) = self.request(req, lease_id)?;
        let record = self.api.create_sandbox(&new).await?;
        let emitter = self.emitter(lease_id);
        let sandbox_id = match self.prepare(&record, clone.as_ref(), emitter.as_ref()).await {
            Ok(sandbox_id) => sandbox_id,
            Err(error) => {
                // The sandbox exists and costs money; a failed delete leaves it for the authority sweep to find.
                let _ = self.api.delete_sandbox(&record.id).await;
                return Err(error);
            }
        };
        self.active.lock().await.insert(
            lease_id.as_str().to_owned(),
            ActiveLease {
                id: record.id,
                region: record.target.or_else(|| req.region.clone()),
            },
        );
        Ok(ProviderLease {
            epoch: None,
            sandbox_id,
            evidence_subject: subjects::evidence_subject(lease_id),
            worker_id: self.worker_id.get().cloned(),
            attempt_id: None,
        })
    }

    /// Deletes every native snapshot still owned by `sandbox`, before the sandbox itself goes. Returns the API to
    /// delete the sandbox with. A worker without snapshot custody has taken no snapshots.
    async fn clean_native_snapshots(&self, sandbox: &str) -> Result<DaytonaApi, DaytonaProviderError> {
        let Some(custody) = &self.snapshot_custody else {
            return Ok(self.api.clone());
        };
        let gate = custody.gate(sandbox);
        let _held = gate.lock().await;
        Ok(custody.cleanup(&self.api, sandbox).await?)
    }

    /// Settles a sandbox this worker does not hold. Only a 404 confirms it is gone, and with custody only a source
    /// custody has an identity for may be settled: anything else belongs to someone else.
    async fn purge_unheld(&self, sandbox: &str) -> Result<PurgeOutcome, DaytonaProviderError> {
        let Some(custody) = &self.snapshot_custody else {
            return match self.api.find_sandbox(sandbox).await? {
                None => Ok(PurgeOutcome::Settled),
                Some(_) => Ok(PurgeOutcome::Stale),
            };
        };
        if !custody.has_source_identity(sandbox).await? {
            return Ok(PurgeOutcome::Stale);
        }
        let api = custody.source_cleanup_api(&self.api, sandbox).await?;
        if api.find_sandbox(sandbox).await?.is_some() {
            return Ok(PurgeOutcome::Stale);
        }
        self.clean_native_snapshots(sandbox).await?;
        Ok(PurgeOutcome::Settled)
    }

    async fn release(&self, lease_id: &LeaseId) -> Result<(), DaytonaProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let id = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .map(|held| held.id.clone());
        let Some(id) = id else {
            return Ok(());
        };
        let api = self.clean_native_snapshots(&id).await?;
        let emitter = self.emitter(lease_id);
        run_phase(emitter.as_ref(), SandboxPhase::Reclaim, None, || async {
            api.delete_sandbox(&id).await.map(drop)
        })
        .await?;
        self.active.lock().await.remove(lease_id.as_str());
        self.exec.forget_lease(lease_id.as_str()).await;
        Ok(())
    }
}

#[async_trait]
impl Provider for DaytonaWorkerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Daytona
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        Ok(self.launch(req, lease_id).await?)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.release(lease_id).await.map_err(ProviderError::backend)
    }
}

#[async_trait]
impl AuthorityHost for DaytonaWorkerProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.active
            .lock()
            .await
            .iter()
            .map(|(lease, held)| {
                let resource_id = AuthorityResourceId::new(held.id.clone())
                    .map_err(|error| AuthorityHostError::Failed(Box::new(error)))?;
                Ok(HeldLease::Provider {
                    lease_id: LeaseId::new(lease.clone()),
                    provider: ProviderKind::Daytona,
                    resource_id,
                })
            })
            .collect()
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Daytona,
            resource_id,
        } = lease
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let failed = |error: DaytonaApiError| AuthorityHostError::Failed(Box::new(error));
        let custody_failed = |error: DaytonaProviderError| AuthorityHostError::Failed(Box::new(error));
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let exact = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|held| held.id == resource_id.as_str());
        if !exact {
            return self.purge_unheld(resource_id.as_str()).await.map_err(custody_failed);
        }
        let api = self
            .clean_native_snapshots(resource_id.as_str())
            .await
            .map_err(custody_failed)?;
        let removed = api.delete_sandbox(resource_id.as_str()).await.map_err(failed)?;
        self.active.lock().await.remove(lease_id.as_str());
        self.exec.forget_lease(lease_id.as_str()).await;
        Ok(if removed {
            PurgeOutcome::Purged
        } else {
            PurgeOutcome::Settled
        })
    }
}
