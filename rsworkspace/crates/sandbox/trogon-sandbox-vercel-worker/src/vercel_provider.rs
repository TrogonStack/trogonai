use std::collections::BTreeMap;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseError, LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, HeldLease, Provider, ProviderError, PurgeOutcome,
    reject_unsupported_privileged, subjects,
};

use crate::active_lease::{ActiveLease, route_map};
use crate::base::Base;
use crate::constants::MAX_TAG_VALUE_LEN;
use crate::native_options::persistent_option;
use crate::network::translate_network_policy;
use crate::profile::PROFILE;
use crate::sizing::resources_for;
use crate::{NewSandbox, SandboxAndSession, VercelApiError, VercelProviderError, VercelWorkerProvider};

impl VercelWorkerProvider {
    pub(crate) async fn launch(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> Result<ProviderLease, VercelProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.active.lock().await.contains_key(lease_id.as_str()) {
            return Err(VercelProviderError::AlreadyHeld);
        }
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
        if !req.spec.mounts().is_empty() {
            return Err(VercelProviderError::MountsUnsupported);
        }
        if req.spec.no_ttl().is_unbounded() {
            return Err(LeaseError::WouldExceedMaxLifetime {
                max: self.plan.max_session_seconds(),
            }
            .into());
        }
        self.plan.check_lifetime(req.ttl_seconds)?;
        let persistent = persistent_option(req.native.as_ref())?.unwrap_or(true);
        let network_policy = translate_network_policy(req.spec.network_policy())?;
        let resources = resources_for(&req.spec, self.max_vcpus)?;
        let base = Base::for_image(req.spec.image(), &self.default_runtime)?;
        let tags: BTreeMap<String, String> = req.tags.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        if let Some((k, _)) = tags.iter().find(|(_, v)| v.chars().count() > MAX_TAG_VALUE_LEN) {
            return Err(LeaseError::InvalidSpec {
                reason: format!("tag {k:?} is longer than Vercel's {MAX_TAG_VALUE_LEN} characters"),
            }
            .into());
        }
        let mut new = NewSandbox::new(
            self.api.config().project_id().clone(),
            (u64::from(req.ttl_seconds) * 1000).max(1000),
            persistent,
        );
        new.runtime = base.runtime();
        new.image = base.image();
        new.source = base.source();
        new.ports = req.spec.exposed_ports().iter().map(|port| port.get()).collect();
        new.resources = resources;
        new.network_policy = network_policy;
        new.env = req.spec.env().clone();
        new.region = req.region.clone();
        new.tags = tags;
        let SandboxAndSession {
            sandbox,
            session,
            routes,
        } = self.api.create_sandbox(&new).await?;
        let sandbox_id = SandboxId::new(sandbox.name.clone())?;
        self.active.lock().await.insert(
            lease_id.as_str().to_owned(),
            ActiveLease {
                name: sandbox.name,
                session_id: session.id,
                routes: route_map(&routes),
                persistent: sandbox.persistent,
                region: session.region.or_else(|| req.region.clone()),
                gate: gate.clone(),
                stopped: false,
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

    async fn release(&self, lease_id: &LeaseId) -> Result<(), VercelApiError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let name = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .map(|held| held.name.clone());
        let Some(name) = name else {
            return Ok(());
        };
        self.api.delete_sandbox(&name).await?;
        self.active.lock().await.remove(lease_id.as_str());
        self.exec.forget_lease(lease_id.as_str()).await;
        Ok(())
    }
}

#[async_trait]
impl Provider for VercelWorkerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Vercel
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        Ok(self.launch(req, lease_id).await?)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.release(lease_id).await.map_err(ProviderError::backend)
    }
}

#[async_trait]
impl AuthorityHost for VercelWorkerProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.active
            .lock()
            .await
            .iter()
            .map(|(lease, held)| {
                let resource_id = AuthorityResourceId::new(held.name.clone())
                    .map_err(|error| AuthorityHostError::Failed(Box::new(error)))?;
                Ok(HeldLease::Provider {
                    lease_id: LeaseId::new(lease.clone()),
                    provider: ProviderKind::Vercel,
                    resource_id,
                })
            })
            .collect()
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Vercel,
            resource_id,
        } = lease
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let failed = |error: VercelApiError| AuthorityHostError::Failed(Box::new(error));
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let exact = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|held| held.name == resource_id.as_str());
        if !exact {
            return match self.api.get_sandbox(resource_id.as_str(), false).await {
                Err(VercelApiError::NotFound(_)) => Ok(PurgeOutcome::Settled),
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
