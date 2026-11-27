use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseError, LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, HeldLease, Provider, ProviderError, PurgeOutcome,
    reject_unsupported_privileged, subjects,
};

use crate::active_lease::ActiveLease;
use crate::base::Base;
use crate::clone_target::CloneTarget;
use crate::constants::{WORKSPACE_MODE, WORKSPACE_PATH};
use crate::lifecycle::auto_intervals;
use crate::native_options::native_params;
use crate::network::translate_network_policy;
use crate::profile::PROFILE;
use crate::settle::settle;
use crate::{
    DaytonaApiError, DaytonaProviderError, DaytonaWorkerProvider, GitCloneRequest, NewSandbox, SandboxRecord,
    SandboxState,
};

impl DaytonaWorkerProvider {
    fn request(&self, req: &LeaseRequest) -> Result<(NewSandbox, Option<CloneTarget>), DaytonaProviderError> {
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
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
        new.labels = req.tags.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
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
    ) -> Result<SandboxId, DaytonaProviderError> {
        let sandbox_id = SandboxId::new(record.id.clone())?;
        settle(&self.api, &record.id, SandboxState::Started, self.wait).await?;
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
        Ok(sandbox_id)
    }

    async fn launch(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, DaytonaProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.active.lock().await.contains_key(lease_id.as_str()) {
            return Err(DaytonaProviderError::AlreadyHeld);
        }
        let (new, clone) = self.request(req)?;
        let record = self.api.create_sandbox(&new).await?;
        let sandbox_id = match self.prepare(&record, clone.as_ref()).await {
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

    async fn release(&self, lease_id: &LeaseId) -> Result<(), DaytonaApiError> {
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
        self.api.delete_sandbox(&id).await?;
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
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let exact = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|held| held.id == resource_id.as_str());
        if !exact {
            return match self.api.get_sandbox(resource_id.as_str()).await {
                Err(DaytonaApiError::NotFound(_)) => Ok(PurgeOutcome::Settled),
                Ok(_) => Ok(PurgeOutcome::Stale),
                Err(error) => Err(failed(error)),
            };
        }
        let removed = self.api.delete_sandbox(resource_id.as_str()).await.map_err(failed)?;
        self.active.lock().await.remove(lease_id.as_str());
        self.exec.forget_lease(lease_id.as_str()).await;
        Ok(if removed {
            PurgeOutcome::Purged
        } else {
            PurgeOutcome::Settled
        })
    }
}
