use super::*;
use trogon_sandbox_protocol::AuthorityHostError;
use trogon_sandbox_protocol::{AuthorityResourceId, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

fn host_failure(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> AuthorityHostError {
    AuthorityHostError::Failed(error.into())
}

impl DockerWorkerProvider {
    pub(crate) fn authority_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self
            .authority_gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease.as_str()).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease.as_str().to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub async fn authority_held_leases(&self) -> Result<Vec<trogon_sandbox_protocol::HeldLease>, AuthorityHostError> {
        self.active
            .lock()
            .await
            .iter()
            .map(|(lease, record)| {
                if let Some(target) = record.dispatch_target.as_ref().filter(|_| record.dispatch_target_bound) {
                    return Ok(HeldLease::Targeted(trogon_sandbox_contract::LeaseAttemptIdentity {
                        lease_id: LeaseId::new(lease.clone()),
                        provider: ProviderKind::LocalDocker,
                        target: target.clone(),
                    }));
                }
                Ok(HeldLease::Provider {
                    lease_id: LeaseId::new(lease.clone()),
                    provider: ProviderKind::LocalDocker,
                    resource_id: AuthorityResourceId::new(record.container_id.clone()).map_err(host_failure)?,
                })
            })
            .collect()
    }

    pub(super) fn authority_matches(record: &ActiveDockerLease, expected: &trogon_sandbox_protocol::HeldLease) -> bool {
        match expected {
            trogon_sandbox_protocol::HeldLease::Targeted(identity) => {
                identity.provider == ProviderKind::LocalDocker
                    && record.dispatch_target_bound
                    && record.dispatch_target.as_ref() == Some(&identity.target)
            }
            trogon_sandbox_protocol::HeldLease::Provider {
                provider: ProviderKind::LocalDocker,
                resource_id,
                ..
            } => record.dispatch_target.is_none() && record.container_id == resource_id.as_str(),
            _ => false,
        }
    }

    pub(super) fn authority_resource(lease: &trogon_sandbox_protocol::HeldLease) -> Option<(&LeaseId, &str)> {
        match lease {
            trogon_sandbox_protocol::HeldLease::Provider {
                lease_id,
                provider: ProviderKind::LocalDocker,
                resource_id,
            } => Some((lease_id, resource_id.as_str())),
            _ => None,
        }
    }

    pub async fn authority_freeze(
        &self,
        expected: &trogon_sandbox_protocol::HeldLease,
    ) -> Result<trogon_sandbox_protocol::FreezeOutcome, AuthorityHostError> {
        let lease = expected.lease_id();
        let _gate = self.authority_gate(lease).lock_owned().await;
        let record = self.active.lock().await.get(lease.as_str()).cloned();
        let Some(record) = record.filter(|record| Self::authority_matches(record, expected)) else {
            return Ok(FreezeOutcome::Stale);
        };
        let resource = record.container_id.as_str();
        if self.container_is_paused(resource).await.map_err(host_failure)? {
            return Ok(if self.authority_paused.lock().await.contains(expected) {
                FreezeOutcome::Frozen
            } else {
                FreezeOutcome::AlreadyPaused
            });
        }
        self.authority_paused.lock().await.insert(expected.clone());
        self.docker.pause_container(resource).await.map_err(host_failure)?;
        Ok(FreezeOutcome::Frozen)
    }

    pub async fn authority_thaw(
        &self,
        expected: &trogon_sandbox_protocol::HeldLease,
    ) -> Result<trogon_sandbox_protocol::ThawOutcome, AuthorityHostError> {
        let lease = expected.lease_id();
        let _gate = self.authority_gate(lease).lock_owned().await;
        let record = self.active.lock().await.get(lease.as_str()).cloned();
        let Some(record) = record.filter(|record| Self::authority_matches(record, expected)) else {
            return Ok(ThawOutcome::Stale);
        };
        let resource = record.container_id.as_str();
        if !self.authority_paused.lock().await.contains(expected) {
            return Ok(ThawOutcome::NoLongerOwned);
        }
        if !self.container_is_paused(resource).await.map_err(host_failure)? {
            self.authority_paused.lock().await.remove(expected);
            return Ok(ThawOutcome::NoLongerOwned);
        }
        self.docker.unpause_container(resource).await.map_err(host_failure)?;
        self.authority_paused.lock().await.remove(expected);
        Ok(ThawOutcome::Thawed)
    }

    pub async fn authority_purge<F, Fut>(
        &self,
        expected: &trogon_sandbox_protocol::HeldLease,
        finish: F,
    ) -> Result<trogon_sandbox_protocol::PurgeOutcome, AuthorityHostError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        if let trogon_sandbox_protocol::HeldLease::Targeted(identity) = expected {
            let tenant = {
                let _gate = self.authority_gate(&identity.lease_id).lock_owned().await;
                let record = self.active.lock().await.get(identity.lease_id.as_str()).cloned();
                let Some(record) = record.filter(|record| Self::authority_matches(record, expected)) else {
                    return Ok(PurgeOutcome::Stale);
                };
                record.request.tenant.clone()
            };
            let request = trogon_sandbox_contract::ProviderCleanupRequest {
                identity: identity.clone(),
                intent: trogon_sandbox_contract::CancelIntent::Purge,
            };
            return match self.cleanup_targeted(&request, &tenant).await {
                trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
                    identity: cleaned,
                    intent: trogon_sandbox_contract::CancelIntent::Purge,
                } if cleaned == *identity => {
                    finish().await;
                    Ok(PurgeOutcome::Purged)
                }
                trogon_sandbox_contract::ProviderCleanupResponse::Failed { message, .. } => {
                    Err(AuthorityHostError::CleanupFailed { message })
                }
                _ => Err(host_failure(
                    "Docker purge did not confirm its exact targeted assignment",
                )),
            };
        }
        let Some((lease, resource)) = Self::authority_resource(expected) else {
            return Ok(PurgeOutcome::Stale);
        };
        let _gate = self.authority_gate(lease).lock_owned().await;
        let record = self.active.lock().await.get(lease.as_str()).cloned();
        let Some(record) = record.filter(|record| Self::authority_matches(record, expected)) else {
            return match self.docker.inspect_container(resource, None).await {
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                    Ok(PurgeOutcome::Stale)
                }
                Err(error) => Err(host_failure(error)),
                Ok(_) => Err(host_failure("exact Docker cleanup receipt is no longer held")),
            };
        };
        match self.docker.inspect_container(resource, None).await {
            Ok(info) if info.state.as_ref().and_then(|state| state.paused).unwrap_or(false) => {
                self.docker.unpause_container(resource).await.map_err(host_failure)?;
            }
            Ok(_) | Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(error) => return Err(host_failure(error)),
        }
        if let Some(sidecar) = &record.sidecar_id {
            self.inner.engine().remove(sidecar).await.map_err(host_failure)?;
        }
        self.teardown_container(lease, resource, None)
            .await
            .map_err(host_failure)?;
        self.inner.engine().remove(resource).await.map_err(host_failure)?;
        match self.docker.inspect_container(resource, None).await {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(error) => return Err(host_failure(error)),
            Ok(_) => return Err(host_failure("Docker resource remains after exact purge")),
        }
        self.active.lock().await.remove(lease.as_str());
        self.authority_paused.lock().await.remove(expected);
        self.forget_executions(lease).await;
        self.rollback_state.forget(lease).await;
        finish().await;
        Ok(PurgeOutcome::Purged)
    }
}
